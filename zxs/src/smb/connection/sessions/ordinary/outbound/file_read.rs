//! Actual confirmed ordinary or retained FILE range. No name lookup or alternate
//! physical read on an already opened handle. A short charge gains no work.
use super::{CheckedBatch,Connection,Fault,Payload,Result,SyncReply,UnsignedSmall,VerifiedMember,current,
    root_io::RequestSlot};
use super::super::opens::Opened;
use crate::smb::{backend::{FileReadError,PreparedFileRead,PreparedRetainedRead,PreparedWriterRead,RangeIoError,ReadCompletion,ReadFault},
    wire::{Error as WireError,direct::MAX_FRAME_BYTES,io_fields::{IoFieldError,OpenReference,WireOpenId},message::{Message,RELATED,Routing},
        read_reply::{HEADER_AND_READ_FIXED,READ,ReadFailure,UnsignedRead}}};
use std::{io,num::NonZeroU64};

impl CheckedBatch<'_,'_>{
    pub(in crate::smb) fn try_send_file_read(&mut self)->Result<bool>{
        self.before_reply()?;
        let frame=self.original.frame;
        let [m]=frame.messages() else{return Ok(false);};
        let Routing::Sync{tree:tid}=m.header.routing else{return Ok(false);};
        if m.header.command!=READ||m.header.flags&RELATED!=0{return Ok(false);}
        let sid=self.ids.at(0).ok_or(Fault::SessionState)?;
        let original=RequestSlot{sid,tree:tid,mid:m.header.message};
        let payload=self.original.payload(0).ok_or(Fault::SessionState)?;
        let draft={
            let (session,trees,opens)=self.owner.member_tables(sid,&self.revision)?;
            // §3.3.5.12: original Session, Tree, then both OPEN halves.
            let Some(tree)=trees.root(tid)else{return self.read_refusal(m,sid,ReadFailure::NetworkNameDeleted);};
            let ids=match m.read_target(){
                Ok(OpenReference::Explicit(v))=>v,
                Err(IoFieldError::NoExplicitOpen)=>return self.read_refusal(m,sid,ReadFailure::FileClosed),
                _=>return self.read_refusal(m,sid,ReadFailure::InvalidParameter),
            };
            let Some(slot)=opens.find_mut(ids).filter(|s|s.tree()==tid)else{
                return self.read_refusal(m,sid,ReadFailure::FileClosed);
            };
            let permitted=match slot.object(){
                Opened::Root(r)=>r.allows_list_directory(),
                Opened::Directory(d)=>d.allows_list_directory(),
                Opened::File(f)=>f.allows_read_data(),
                Opened::Writer(f)=>f.allows_read_data(),
                Opened::Retained(f)=>f.allows_read_data(),
            };
            if !permitted{return self.read_refusal(m,sid,ReadFailure::AccessDenied);}
            match payload{
                Payload::Covered=>{},
                Payload::InvalidParameter=>return self.read_refusal(m,sid,ReadFailure::InvalidParameter),
                _=>return Err(Fault::SessionState),
            };
            let member=VerifiedMember{message:m,ordinal:0,session};
            if m.read_request(member.max_read()).is_err(){return self.read_refusal(m,sid,ReadFailure::InvalidParameter);}
            // MaxReadSize is independent of MaxTransactSize. Reserve the
            // fixed response bytes inside the absolute Direct TCP frame cap.
            // A writer reads its own staged content, never a different
            // confirmed read handle or an arbitrary earlier version.
            let room=MAX_FRAME_BYTES-HEADER_AND_READ_FIXED;
            let result=match slot.object_mut(){
                Opened::File(file)=>file.read(tree,&member,room).map(Prepared::File),
                Opened::Writer(file)=>file.read(tree,&member,room).map(Prepared::Writer),
                Opened::Retained(open) if !open.is_directory()=>open.read(tree,&member,room).map(Prepared::Retained),
                Opened::Retained(_)=>return self.read_refusal(m,sid,ReadFailure::FileIsDirectory),
                Opened::Root(_)|Opened::Directory(_)=>return self.read_refusal(m,sid,ReadFailure::FileIsDirectory),
            };
            let prepared=match result{
                Ok(v)=>v,
                Err(e)=>return self.read_refusal(m,sid,read_failure(e)?),
            };
            match UnsignedRead::from_original(&member,PendingRead{original,ids,prepared}){
                Ok(v)=>v,
                Err(WireError::Limit)=>return self.read_refusal(m,sid,ReadFailure::Resources),
                Err(_)=>return Err(Fault::WrongReply),
            }
        };
        self.send_file_read(draft)?;Ok(true)
    }
    fn read_refusal(&mut self,m:&Message<'_>,sid:NonZeroU64,why:ReadFailure)->Result<bool>{
        self.owner.member_scope(sid,&self.revision)?;
        let draft=UnsignedSmall::read_failure(&ReadRefusal{message:m,sid,why}).map_err(|_|Fault::WrongReply)?;
        self.next=1;self.small_replies(std::iter::once(draft))?.send()?;Ok(true)
    }
    fn send_file_read(&mut self,draft:UnsignedRead)->Result<()>{
        self.next=1;self.halted=true;
        let ids=&self.ids;let revision=&self.revision;
        let plan=self.owner.plan_debit(&mut self.original.debit)?;
        let payable=draft.credit(plan.credit_response(0).ok_or(Fault::NoReply)?);
        let sid=payable.sid();
        if ids.at(0)!=Some(sid){return Err(Fault::SessionState);}
        plan.owner.member_scope(sid,revision)?;
        let (_,bound)=plan.owner.valid_slot(sid).ok_or(Fault::SessionState)?;
        let (record,pending)=bound.sign_file_read(revision,payable).map_err(|_|Fault::SessionState)?.into_send();
        let tx=plan.bind_record(record)?;
        SyncReply{tx,ids,revision,count:1}.write_file_read(pending)
    }
}

fn read_failure(e:FileReadError)->Result<ReadFailure>{
    use ReadFailure as W;
    Ok(match e{
        FileReadError::Fields(_)=>W::InvalidParameter,
        FileReadError::Session(e)=>return Err(e),
        FileReadError::WrongTree=>W::NetworkNameDeleted,
        FileReadError::Fault(e)=>match e{
            ReadFault::AccessDenied=>W::AccessDenied,
            ReadFault::NotDataFile=>W::FileIsDirectory,
            ReadFault::UnbufferedUnavailable=>W::NotSupported,
            ReadFault::ResponseSpace=>W::Resources,
            ReadFault::InconsistentLength=>W::Internal,
            ReadFault::Range(e)=>match e{
                RangeIoError::Conflict=>W::FileLockConflict,
                RangeIoError::Resources=>W::Resources,
                RangeIoError::Unavailable=>W::ServerUnavailable,
            },
            // EOF is permissible only with the original current held view
            // and late proof in PreparedFileRead, not a plain error result.
            ReadFault::EndOfFile=>return Err(Fault::WrongReply),
            ReadFault::Original(e)=>match e.kind(){
                io::ErrorKind::NotConnected=>W::VolumeDismounted,
                io::ErrorKind::NotFound|io::ErrorKind::WouldBlock=>W::FileNotAvailable,
                io::ErrorKind::PermissionDenied=>W::AccessDenied,
                io::ErrorKind::OutOfMemory=>W::Resources,
                _=>W::ServerUnavailable,
            },
        },
    })
}

pub(in crate::smb) struct ReadRefusal<'a,'r>{message:&'a Message<'r>,sid:NonZeroU64,why:ReadFailure}
impl<'a,'r> ReadRefusal<'a,'r>{
    pub(in crate::smb) fn fields(&self)->(&'a Message<'r>,NonZeroU64,ReadFailure){(self.message,self.sid,self.why)}
}
#[must_use]
pub(in crate::smb) struct PendingRead {original:RequestSlot,ids:WireOpenId,prepared:Prepared}
enum Prepared {File(PreparedFileRead),Writer(PreparedWriterRead),Retained(PreparedRetainedRead)}
impl PendingRead {
    pub(in crate::smb) fn sid(&self)->NonZeroU64{self.original.sid}
    pub(in crate::smb) fn matches(&self,m:&VerifiedMember<'_,'_>)->bool{self.original.matches(m,self.sid())}
    pub(in crate::smb) fn result(&self)->&std::result::Result<ReadCompletion<Vec<u8>>,ReadFault<io::Error>>{
        match &self.prepared{
            Prepared::File(p)=>p.result(),Prepared::Writer(p)=>p.result(),Prepared::Retained(p)=>p.result(),
        }
    }
    fn check(&self,c:&Connection)->Result<()>{
        let tree=self.original.root(c)?;
        let slot=c.open_table(self.sid())?.find(self.ids).filter(|s|s.tree()==self.original.tree).ok_or(Fault::SessionState)?;
        match (&self.prepared,slot.object()){
            (Prepared::File(p),Opened::File(file))=>p.require_current(file,tree),
            (Prepared::Writer(p),Opened::Writer(file))=>p.require_current(file,tree),
            (Prepared::Retained(p),Opened::Retained(file))=>p.require_current(file,tree),
            _=>return Err(Fault::SessionState),
        }.map_err(|_|Fault::SessionState)
    }
}
impl SyncReply<'_>{
    fn write_file_read(self,pending:PendingRead)->Result<()>{
        pending.check(self.tx.plan.owner)?;
        let SyncReply{tx,ids,revision,count}=self.write_original()?.0;
        // Both data and genuine EOF keep their exact original physical proof
        // until completion. No cursor or other business state is modified.
        tx.publish_with(|c|{current(c,ids,revision,count)?;pending.check(c)})
    }
}
