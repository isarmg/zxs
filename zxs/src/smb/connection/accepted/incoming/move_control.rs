//! The original peer's control stream while one move is pending.
use super::{Arc,CapturedRecord,Connection,Fault,MoveFlight,Peer,PendingOriginalMove,Result,frame_limit};
use crate::smb::{connection::{sessions::ordinary::MoveCancellation,transport::PendingRead},wire::message::CANCEL};
use std::{collections::VecDeque,os::fd::BorrowedFd};

// The shipping dialect advertises 64 KiB MaxTransactSize. Keep at most
// the existing per-connection credit count of actual-size records, with
// an independent aggregate byte cap and one separately bounded partial.
const BACKLOG_RECORDS:usize=crate::smb::connection::CAP as usize;
const BACKLOG_BYTES:usize=BACKLOG_RECORDS*(65_536+256+4);
#[derive(Default)]
pub(in crate::smb::connection::accepted) struct MoveControl{
    input:PendingRead,deferred:VecDeque<CapturedRecord>,deferred_bytes:usize,
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) enum ControlTurn{Waiting,NoTarget,OrdinaryHeld,Cancelled,CancelRequested,TooLate,WorkerFinished,MoveSettled}
pub(super) enum ControlInput{Waiting,OrdinaryHeld,Cancel(CapturedRecord)}

impl CapturedRecord{
    pub(super) fn from_pending(owner:&mut Connection,read:&mut PendingRead)->Result<Option<Self>>{
        let limit=frame_limit(owner);
        let got=read.advance(owner.stream.as_mut().ok_or(Fault::Transport)?,limit)?;
        Ok(got.map(|bytes|Self{origin:Arc::downgrade(&owner.origin),bytes}))
    }
}
impl MoveControl{
    pub(super) fn empty(&self)->bool{self.deferred.is_empty()&&!self.input.has_partial()}
    fn remember(&mut self,record:CapturedRecord)->Result<()>{
        let bytes=self.deferred_bytes.checked_add(record.bytes.len()).ok_or(Fault::Resources)?;
        if self.deferred.len()>=BACKLOG_RECORDS||bytes>BACKLOG_BYTES{return Err(Fault::Resources);}
        self.deferred.try_reserve(1).map_err(|_|Fault::Resources)?;
        self.deferred.push_back(record);self.deferred_bytes=bytes;Ok(())
    }
    fn oldest(&mut self)->Option<CapturedRecord>{
        let first=self.deferred.pop_front()?;
        self.deferred_bytes-=first.bytes.len();Some(first)
    }
    /// After the original has settled, resume exactly the same stream:
    /// oldest complete frame first, then the already consumed prefix/body.
    /// Neither path has debited or executed an ordinary request.
    pub(super) fn resume(&mut self,c:&mut Connection,limit:usize)->Result<Option<CapturedRecord>>{
        if let Some(record)=self.oldest(){record.require_owner(c)?;return Ok(Some(record));}
        if !self.input.has_partial(){return Ok(None);}
        let bytes=self.input.finish_pending(c.stream.as_mut().ok_or(Fault::Transport)?,limit)?
            .ok_or(Fault::WrongBatch)?;
        Ok(Some(CapturedRecord{origin:Arc::downgrade(&c.origin),bytes}))
    }
    pub(super) fn receive(&mut self,c:&mut Connection)->Result<ControlInput>{
        c.alive()?;
        // Observe EOF even with a backlog. Frame one bounded receive step,
        // never wait on ordinary application work to see a following CANCEL.
        c.stream.as_ref().ok_or(Fault::Transport)?.require_pending_peer()?;
        let Some(record)=CapturedRecord::from_pending(c,&mut self.input)?else{return Ok(ControlInput::Waiting);};
        if record.frame_on(c)?.messages().iter().any(|m|m.header.command==CANCEL){
            return Ok(ControlInput::Cancel(record));
        }
        // A whole ordinary record stays in arrival order, with no debit or
        // business side effect. Never scan inside it or beyond its framing.
        self.remember(record)?;
        Ok(ControlInput::OrdinaryHeld)
    }
    pub(super) fn wait_for_worker_or_input(&self,c:&Connection,completion:BorrowedFd<'_>)->Result<()>{
        c.alive()?;
        self.input.wait_for_worker_or_input(c.stream.as_ref().ok_or(Fault::Transport)?,completion)
    }
}
impl PendingOriginalMove{
    fn control_once(&mut self,c:&mut Connection)->Result<ControlTurn>{
        self.held.require_session(c)?;
        let record=match self.control.receive(c)?{
            ControlInput::Waiting=>return Ok(ControlTurn::Waiting),
            ControlInput::OrdinaryHeld=>return Ok(ControlTurn::OrdinaryHeld),
            ControlInput::Cancel(r)=>r,
        };
        match self.held.cancel_before_work(c,&self.input,&record)?{
            MoveCancellation::NoTarget=>Ok(ControlTurn::NoTarget),
            MoveCancellation::Written=>Ok(ControlTurn::Cancelled),
        }
    }
}
impl Peer{
    pub(super) fn poll_original_move_control(&mut self)->Result<ControlTurn>{
        let turn=match self.original_move.as_mut().ok_or(Fault::WrongBatch)?{
            MoveFlight::Unstarted(held)=>held.control_once(&mut self.connection),
            MoveFlight::Working(held)=>held.control_once(&mut self.connection),
        };
        match turn{
            Err(e)=>{self.disconnect_original();Err(e)},
            Ok(turn @ (ControlTurn::Cancelled|ControlTurn::MoveSettled))=>{
                // The original requested final has already been completely
                // written and settled. Join any real stopped worker before
                // returning retained normal frames to the same ordinary path.
                let backlog=match self.original_move.take(){
                    Some(MoveFlight::Unstarted(held)) if turn==ControlTurn::Cancelled=>held.control,
                    Some(MoveFlight::Working(mut held))=>held.take_control_after_terminal(),
                    _=>{self.disconnect_original();return Err(Fault::WrongBatch);},
                };
                if !backlog.empty(){self.move_backlog=Some(backlog);}
                Ok(turn)
            },
            Ok(other)=>Ok(other),
        }
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn parked_ordinary_buffers_have_fixed_count_and_fifo(){
        fn record(n:u8)->CapturedRecord{CapturedRecord{origin:super::super::Weak::new(),bytes:vec![n]}}
        let mut control=MoveControl::default();
        for n in 0..BACKLOG_RECORDS{control.remember(record(n as u8)).unwrap();}
        assert_eq!(control.remember(record(77)),Err(Fault::Resources));
        assert_eq!(control.deferred_bytes,BACKLOG_RECORDS);
        for n in 0..BACKLOG_RECORDS{assert_eq!(control.oldest().unwrap().bytes,[n as u8]);}
        assert!(control.empty());
        let wide=CapturedRecord{origin:super::super::Weak::new(),bytes:vec![0;BACKLOG_BYTES+1]};
        assert_eq!(control.remember(wide),Err(Fault::Resources));
        assert!(control.empty());
    }
}
