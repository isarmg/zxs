//! The original charged SET_INFO owns its interim and terminal response.
//! The mover runs only its bound storage operation, never the wire.
mod async_turn;
pub(in crate::smb::connection) use async_turn::{CheckWrittenMove,MoveWritten};
use super::{CheckedBatch,Connection,Fault,Payload,VerifiedMember,root_io::RequestSlot};
use super::super::{cancel_shape,signed_cancel,opens::Opened};
use crate::smb::{backend::{MoveOpenWitness,OrdinaryRenameError,PreparedCrossFilesystemMove,WorkingMoveGuard},
    connection::{MoveDebt,Received,Result as PeerResult,accepted::incoming::{CapturedRecord,MoveCapture,OriginalMoveInput}},
    session::{self,BatchRevision,VerifiedIds},wire::{fixed_reply::UnsignedSmall,io_fields::{IoFieldError,OpenReference,WireOpenId},
        message::{Compound,Message,RELATED,Routing},set_info::{RenameError,RenameRequest}}};
use crate::system::reader::smb::{ConfirmedOrdinaryMove,first_work::{RustAdoptedMove,RustOrdinaryMove}};
use std::{num::NonZeroU64,sync::Weak};

#[derive(Debug)]
pub(in crate::smb) enum MoveQualificationError{
    Connection(Fault),Malformed,FileClosed,Fields(RenameError),Original(OrdinaryRenameError),
}

#[must_use]
pub(in crate::smb) struct PreparedOriginalMove{
    request:RequestSlot,open:WireOpenId,_held:PreparedCrossFilesystemMove,
}

impl PreparedOriginalMove{
    fn require_open(&self,owner:&Connection,member:&VerifiedMember<'_,'_>)->PeerResult<()>{
        let tree=self.request.root(owner)?;
        let slot=owner.open_table(self.request.sid)?.find(self.open)
            .filter(|slot|slot.tree()==self.request.tree).ok_or(Fault::SessionState)?;
        let Opened::File(file)=slot.object()else{return Err(Fault::SessionState);};
        let witness=member.rust_session_witness()?;
        self._held.require_original_open(file,tree,&witness).map_err(|_|Fault::SessionState)
    }
}

/// Only the closed wire codec borrows this proof. It cannot produce an
/// SMB2 CANCEL response, a success, or a result for another original MID.
#[must_use]
pub(in crate::smb) struct CancelledOriginalMove<'a>{request:&'a RequestSlot,open:WireOpenId,async_id:Option<NonZeroU64>}
impl CancelledOriginalMove<'_>{
    pub(in crate::smb) fn matches(&self,member:&VerifiedMember<'_,'_>,sid:NonZeroU64)->bool{
        move_reply_matches(member,sid,self.request,self.open)
    }
    pub(in crate::smb) fn async_id(&self)->Option<NonZeroU64>{self.async_id}
}
/// Only the closed success codec consumes this already checked original.
#[must_use]
pub(in crate::smb) struct ConfirmedOriginalMove<'a>{request:&'a RequestSlot,open:WireOpenId,async_id:NonZeroU64}
impl ConfirmedOriginalMove<'_>{
    pub(in crate::smb) fn matches(&self,member:&VerifiedMember<'_,'_>,sid:NonZeroU64)->bool{
        move_reply_matches(member,sid,self.request,self.open)
    }
    pub(in crate::smb) fn async_id(&self)->NonZeroU64{self.async_id}
}
fn move_reply_matches(member:&VerifiedMember<'_,'_>,sid:NonZeroU64,request:&RequestSlot,open:WireOpenId)->bool{
    member.ordinal()==0&&request.matches(member,sid)&&member.message().is_basic_rename_request()&&
        member.message().set_info_target().is_ok_and(|id|id==OpenReference::Explicit(open))
}

/// An unstarted escrow cannot run while still holding its pre-work writer.
/// Worker admission must consume
/// this type into a different state, and never call its pre-work finalizer.
/// The still-owed debt invalidates only the original before the guard drops.
#[must_use]
pub(in crate::smb::connection) struct MoveEscrow{wire:MoveWire,prepared:PreparedOriginalMove}
struct MoveWire{
    debt:MoveDebt,ids:VerifiedIds,revision:BatchRevision,input:Weak<()>,
}
/// Distinct owner: never reaches the unstarted cancellation writer. The
/// original debt stays until actual cancellation or after-write proof.
#[must_use]
pub(in crate::smb::connection) struct WorkingMoveEscrow{
    wire:MoveWire,request:RequestSlot,open:WireOpenId,_guard:WorkingMoveGuard,final_started:bool,
}
pub(in crate::smb::connection) enum MoveCancellation{NoTarget,Written}
/// A non-move retains its exact paid batch for ordinary dispatch.
#[must_use]
pub(in crate::smb::connection) enum MoveDecision<'owner,'raw>{
    Ordinary(CheckedBatch<'owner,'raw>),Park(MoveEscrow),RefusalWritten,
}

impl MoveWire{
    fn require_session(&self,owner:&Connection,request:&RequestSlot)->PeerResult<()>{
        owner.member_scope(request.sid,&self.revision).map(drop)
    }
    fn original<'a>(&self,owner:&Connection,input:&'a OriginalMoveInput,
        request:&RequestSlot,open:WireOpenId)->PeerResult<Compound<'a>>{
        if !input.issued(&self.input){return Err(Fault::WrongBatch);}
        let frame=input.frame_on(owner)?;
        let [raw]=frame.messages()else{return Err(Fault::WrongBatch);};
        self.debt.require(owner,raw)?;
        let sid=self.ids.at(0).ok_or(Fault::SessionState)?;
        let checked=session::verify_published(&frame,&self.revision,|id|owner.published_bound(id))
            .map_err(|_|Fault::SessionState)?;
        if checked.at(0)!=Some(sid){return Err(Fault::SessionState);}
        let scope=owner.member_scope(sid,&self.revision)?;
        let member=VerifiedMember{message:raw,ordinal:0,session:scope};
        if !request.matches(&member,sid)||
            !raw.set_info_target().is_ok_and(|id|id==OpenReference::Explicit(open)){
            return Err(Fault::WrongBatch);
        }
        Ok(frame)
    }
    fn matches_cancel(&self,owner:&Connection,cancel:&CapturedRecord,request:&RequestSlot)->PeerResult<bool>{
        let control=cancel.frame_on(owner)?;
        let member=cancel_shape(&control)?;
        let sid=signed_cancel(owner,&control,&self.revision)?;
        Ok(targets_original(member,sid,request,self.debt.interim))
    }
}

impl WorkingMoveEscrow{
    pub(in crate::smb::connection) fn require_session(&self,owner:&Connection)->PeerResult<()>{
        self.wire.require_session(owner,&self.request)
    }
    pub(in crate::smb::connection) fn matches_cancel(&self,owner:&Connection,
        input:&OriginalMoveInput,cancel:&CapturedRecord)->PeerResult<bool>{
        self.wire.original(owner,input,&self.request,self.open)?;
        self.wire.matches_cancel(owner,cancel,&self.request)
    }

    /// Rebind in place on the original live two-half OPEN only. This never
    /// publishes IDs, changes the credit ledger or signs a status.
    pub(in crate::smb::connection) fn adopt_destination(&self,owner:&mut Connection,input:&OriginalMoveInput,
        witness:MoveOpenWitness,target:ConfirmedOrdinaryMove)->PeerResult<RustAdoptedMove>{
        let frame=self.wire.original(owner,input,&self.request,self.open)?;
        let [message]=frame.messages()else{return Err(Fault::WrongBatch);};
        let (session,trees,opens)=owner.member_tables(self.request.sid,&self.wire.revision)?;
        let tree=trees.root(self.request.tree).ok_or(Fault::SessionState)?;
        let slot=opens.find_mut(self.open).filter(|s|s.tree()==self.request.tree).ok_or(Fault::SessionState)?;
        let Opened::File(open)=slot.object_mut()else{return Err(Fault::SessionState);};
        let member=VerifiedMember{message,ordinal:0,session}.rust_session_witness()?;
        self._guard.adopt(open,tree,&member,witness,target).map_err(|_|Fault::WriteOutcomeUnknown)
    }
}

impl MoveEscrow{
    pub(in crate::smb::connection) fn require_session(&self,owner:&Connection)->PeerResult<()>{
        self.wire.require_session(owner,&self.prepared.request)
    }
    pub(in crate::smb::connection) fn require_original(&self,owner:&Connection,input:&OriginalMoveInput)->PeerResult<()>{
        self.wire.original(owner,input,&self.prepared.request,self.prepared.open).map(drop)
    }

    /// Transfer only the already bound single Core operation. The caller
    /// retains the charged wire owner and the same share turn until reaped.
    pub(in crate::smb::connection) fn assign_worker(self,owner:&Connection,input:&OriginalMoveInput)
        ->PeerResult<(WorkingMoveEscrow,RustOrdinaryMove,MoveOpenWitness)>{
        if self.wire.debt.interim.is_none(){return Err(Fault::NoReply);}
        let frame=self.wire.original(owner,input,&self.prepared.request,self.prepared.open)?;
        let [message]=frame.messages()else{return Err(Fault::WrongBatch);};
        let member=VerifiedMember{message,ordinal:0,session:owner.member_scope(self.prepared.request.sid,&self.wire.revision)?};
        self.prepared.require_open(owner,&member)?;
        let Self{wire,prepared:PreparedOriginalMove{request,open,_held}}=self;
        let (job,guard,witness)=_held.hand_to_worker();
        Ok((WorkingMoveEscrow{wire,request,open,_guard:guard,final_started:false},job,witness))
    }

    /// No sequence lookup/debit for CANCEL. Both allocations came from the
    /// original peer. An authenticated but unrelated target is silent.
    pub(in crate::smb::connection) fn cancel_before_work(&mut self,owner:&mut Connection,
        input:&OriginalMoveInput,cancel:&CapturedRecord)->PeerResult<MoveCancellation>
    {
        if self.wire.debt.interim.is_some(){return Err(Fault::WrongReply);}
        let frame=self.wire.original(owner,input,&self.prepared.request,self.prepared.open)?;
        if !self.wire.matches_cancel(owner,cancel,&self.prepared.request)?{return Ok(MoveCancellation::NoTarget);}
        self.send_cancelled(owner,&frame)?;
        Ok(MoveCancellation::Written)
    }

    // Reached only through the current signed original CANCEL above. Check
    // the sole OPEN/turn without SQL/media I/O before and after the original
    // stream write. Both deadline and actual TCP checks are transport-local.
    fn send_cancelled(&mut self,owner:&mut Connection,frame:&Compound<'_>)->PeerResult<()>{
        let [message]=frame.messages()else{return Err(Fault::WrongBatch);};
        let prepared=&self.prepared;
        let sid=prepared.request.sid;
        let revision=&self.wire.revision;
        let check_open=|c:&Connection|{
            let member=VerifiedMember{message,ordinal:0,session:c.member_scope(sid,revision)?};
            prepared.require_open(c,&member)
        };
        check_open(owner)?;
        let member=VerifiedMember{message,ordinal:0,session:owner.member_scope(sid,revision)?};
        let proof=CancelledOriginalMove{request:&prepared.request,open:prepared.open,async_id:None};
        let draft=UnsignedSmall::cancelled_move(&member,sid,&proof).map_err(|_|Fault::WrongReply)?;
        let plan=owner.plan_debit(&mut self.wire.debt.original)?;
        let grant=plan.credit_response(0).ok_or(Fault::NoReply)?;
        let (_,bound)=plan.owner.valid_slot(sid).ok_or(Fault::SessionState)?;
        let signed=bound.sign_small_reply(revision,draft.credit(grant)).map_err(|_|Fault::SessionState)?;
        let bytes=signed.bytes();
        let mut record=Vec::new();
        record.try_reserve_exact(4+bytes.len()).map_err(|_|Fault::Resources)?;
        record.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        record.extend_from_slice(bytes);
        let tx=plan.bind_record(record)?;
        let mut stream=tx.plan.owner.stream.take().ok_or(Fault::Transport)?;
        let c:&Connection=&*tx.plan.owner;
        stream.require_pending_peer()?;
        stream.write_record(tx.actual.as_ref(),||c.member_scope(sid,revision).map(drop))?;
        tx.plan.owner.stream=Some(stream);
        // In particular, seeing FIN or credential/OPEN loss here after the
        // last write closes just this stream rather than issuing a grant.
        tx.publish_with(|c|{
            check_open(c)?;
            c.stream.as_ref().ok_or(Fault::Transport)?.require_pending_peer()
        })
    }
}

/// Standard sync CANCEL can cross the interim in flight. The async form
/// needs the issued AsyncId; its MID is not a target lookup key.
fn targets_original(m:&Message<'_>,sid:NonZeroU64,original:&RequestSlot,interim:Option<NonZeroU64>)->bool{
    sid==original.sid&&match m.header.routing{
        Routing::Sync{..}=>m.header.message==original.mid,
        Routing::AsyncCancel{id}=>interim.is_some_and(|issued|issued.get()==id),
    }
}

impl<'owner,'raw> CheckedBatch<'owner,'raw>{
    pub(in crate::smb::connection) fn try_ordinary_move(mut self,capture:MoveCapture<'_,'_>)
        ->PeerResult<MoveDecision<'owner,'raw>>{
        use self::MoveQualificationError as E;
        use super::restore::{ordinary_rename_failure,parse_failure};
        use crate::smb::wire::fixed_reply::RestoreFailure as W;
        if !capture.matches(self.original.frame){return Err(Fault::WrongBatch);}
        let status=match self.qualify_ordinary_cross_filesystem_move(){
            Ok(Some(move_))=>return self.retain_prepared_move(capture,move_).map(MoveDecision::Park),
            Ok(None)=>return Ok(MoveDecision::Ordinary(self)),
            Err(E::Original(e))=>ordinary_rename_failure(e)?,
            Err(E::Fields(e))=>parse_failure(e)?,
            Err(E::Malformed)=>W::InvalidParameter,
            Err(E::FileClosed)=>W::FileClosed,
            Err(E::Connection(e))=>return Err(e),
        };
        let frame=self.original.frame;
        let [m]=frame.messages()else{return Err(Fault::WrongBatch);};
        let sid=self.ids.at(0).ok_or(Fault::SessionState)?;
        self.rename_refusal(m,sid,status)?;
        Ok(MoveDecision::RefusalWritten)
    }
    #[cfg(test)]
    pub(in crate::smb::connection) fn into_move_escrow(mut self,capture:MoveCapture<'_,'_>)->PeerResult<MoveEscrow>{
        if !capture.matches(self.original.frame){return Err(Fault::WrongBatch);}
        let prepared=self.qualify_ordinary_cross_filesystem_move().map_err(|e|match e{
            MoveQualificationError::Connection(e)=>e,
            _=>Fault::WrongReply,
        })?.ok_or(Fault::WrongReply)?;
        self.retain_prepared_move(capture,prepared)
    }
    fn retain_prepared_move(self,capture:MoveCapture<'_,'_>,prepared:PreparedOriginalMove)->PeerResult<MoveEscrow>{
        if !capture.matches(self.original.frame){return Err(Fault::WrongBatch);}
        let Self{owner,original:Received{frame,debit},ids,revision,..}=self;
        let [raw]=frame.messages()else{return Err(Fault::WrongBatch);};
        let debt=owner.retain_move_debt(debit,raw)?;
        Ok(MoveEscrow{wire:MoveWire{debt,ids,revision,input:capture.seal()},prepared})
    }
    /// Qualification alone changes no credit, material or receipt.
    fn qualify_ordinary_cross_filesystem_move(&mut self)
        ->Result<Option<PreparedOriginalMove>,MoveQualificationError>{
        use MoveQualificationError as E;
        self.before_reply().map_err(E::Connection)?;
        let [raw]=self.original.frame.messages()else{return Ok(None);};
        let Routing::Sync{tree:tid}=raw.header.routing else{return Ok(None);};
        if raw.header.flags&RELATED!=0||!raw.is_basic_rename_request(){return Ok(None);}
        let sid=self.ids.at(0).ok_or(E::Connection(Fault::SessionState))?;
        let (session,trees,opens)=self.owner.member_tables(sid,&self.revision).map_err(E::Connection)?;
        let tree=trees.root(tid).ok_or(E::Original(OrdinaryRenameError::WrongTree))?;
        let ids=match raw.set_info_target(){
            Ok(OpenReference::Explicit(ids))=>ids,
            Err(IoFieldError::NoExplicitOpen)=>return Err(E::FileClosed),
            _=>return Err(E::Malformed),
        };
        let slot=opens.find(ids).filter(|slot|slot.tree()==tid).ok_or(E::FileClosed)?;
        let Opened::File(open)=slot.object()else{return Ok(None);};
        match self.original.payload(0){
            Some(Payload::Covered)=>{},
            Some(Payload::InvalidParameter)=>return Err(E::Malformed),
            _=>return Err(E::Connection(Fault::SessionState)),
        }
        let member=VerifiedMember{message:raw,ordinal:0,session};
        let request=RenameRequest::parse(member).map_err(|(_,why)|E::Fields(why))?;
        if request.open()!=OpenReference::Explicit(ids){return Err(E::Connection(Fault::WrongReply));}
        let held=open.prepare_cross_filesystem_move(tree,&request).map_err(E::Original)?;
        Ok(held.map(|_held|PreparedOriginalMove{request:RequestSlot{sid,tree:tid,mid:raw.header.message},open:ids,_held}))
    }
}


#[cfg(test)]
mod tests{
    use super::*;
    use crate::smb::wire::{direct::Frame,message::CANCEL};

    #[test]
    fn before_interim_cancel_matches_the_original_mid_and_verified_sid_not_the_tree_or_async_field(){
        // Routing only. The real receiver independently authenticates the
        // complete original CMAC; this cannot produce a cancellation proof.
        let sid=NonZeroU64::new(17).unwrap();
        let request=RequestSlot{sid,tree:71,mid:13};
        for (mid,flags,routing,pre,post) in [(13u64,8u8,0u64,true,true),(13,8,999u64<<32,true,true),
            (14,8,71u64<<32,false,false),(13,10,13,false,false),(0,10,14,false,true),(0,10,0,false,false)]
        {
            let mut p=[0u8;72];
            p[..4].copy_from_slice(&68u32.to_be_bytes());
            p[4..8].copy_from_slice(b"\xfeSMB");
            p[8]=64;p[16..18].copy_from_slice(&CANCEL.to_le_bytes());p[20]=flags;
            p[28..36].copy_from_slice(&mid.to_le_bytes());
            p[36..44].copy_from_slice(&routing.to_le_bytes());
            p[44..52].copy_from_slice(&sid.get().to_le_bytes());
            p[68..70].copy_from_slice(&4u16.to_le_bytes());
            p[70]=0x6a; // ignored reserved field
            let parsed=Compound::parse(Frame::split(&p).unwrap().unwrap().0).unwrap();
            let m=cancel_shape(&parsed).unwrap();
            assert_eq!(targets_original(m,sid,&request,None),pre);
            assert_eq!(targets_original(m,sid,&request,NonZeroU64::new(14)),post);
            assert!(!targets_original(m,NonZeroU64::new(18).unwrap(),&request,NonZeroU64::new(14)));
        }
    }
}
