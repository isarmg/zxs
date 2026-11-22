//! The one long standalone SET_INFO may grant credits only in the fully
//! submitted interim. Its original debt remains until a genuine terminal
//! result, with independent readback around a selected-target response.
use super::{CancelledOriginalMove,ConfirmedOriginalMove,Connection,Fault,Message,MoveEscrow,Opened,OriginalMoveInput,PeerResult,UnsignedSmall,
    VerifiedMember};
use super::WorkingMoveEscrow;
use crate::smb::{connection::{Window,accepted::incoming::WorkerCancelled},
    session::BatchRevision,wire::message::SIGNED};
use crate::system::reader::smb::{CheckedMoveTarget,MoveTargetKey,first_work::RustAdoptedMove};
use std::{num::NonZeroU64,sync::{Arc,Weak}};

/// Opaque evidence that one original signed success was submitted, not
/// that the client's application received it or the debt can be settled.
#[must_use]
pub(in crate::smb::connection) struct MoveWritten(Arc<WriteSeal>);
#[must_use]
pub(in crate::smb::connection) struct CheckWrittenMove(Arc<WriteSeal>);
struct WriteSeal{input:Weak<()>,id:NonZeroU64,target:MoveTargetKey}
impl MoveWritten{
    pub(in crate::smb::connection) fn for_readback(&self)->CheckWrittenMove{CheckWrittenMove(Arc::clone(&self.0))}
}
impl CheckWrittenMove{
    pub(in crate::smb::connection) fn target(&self)->&MoveTargetKey{&self.0.target}
}

struct Interim{id:NonZeroU64,window:Window,grant:u16}
impl Interim{
    // MIDs are spent once, never wrap and never include u64::MAX. For
    // this sole async owner MID+1 is unique and nonzero on this transport.
    fn plan(mut window:Window,pending:u16,original:&Message<'_>)->PeerResult<Self>{
        let id=original.header.message.checked_add(1).and_then(NonZeroU64::new).ok_or(Fault::Range)?;
        let grant=window.grant(original.header.credit_request,pending)?;
        Ok(Self{id,window,grant})
    }
    fn bytes(&self,original:&Message<'_>)->[u8;72]{
        // MS-SMB2 §3.3.4.2: unsigned interim only, ASYNC/STATUS_PENDING,
        // original MID/SID/command, ERROR with empty ByteCount and context.
        // The actual terminal remains signed by the exact original key.
        let mut raw=[0;72];
        raw[..64].copy_from_slice(&original.raw[..64]);
        raw[8..12].copy_from_slice(&0x0000_0103u32.to_le_bytes());
        raw[14..16].copy_from_slice(&self.grant.to_le_bytes());
        raw[16..20].copy_from_slice(&((original.header.flags&!SIGNED)|3).to_le_bytes());
        raw[20..24].fill(0);
        raw[32..40].copy_from_slice(&self.id.get().to_le_bytes());
        raw[48..64].fill(0);
        raw[64]=9;
        raw
    }
}

// This helper is local to the two exact codecs below. The original peer,
// exclusively borrowed, is the only writer and stream cannot be replaced.
// No live Claim or share lock is held during a network syscall.
fn send(owner:&mut Connection,bytes:&[u8],sid:NonZeroU64,revision:&BatchRevision)->PeerResult<()>{
    let mut stream=owner.stream.take().ok_or(Fault::Transport)?;
    stream.require_pending_peer()?;
    stream.write_smb(bytes,||owner.member_scope(sid,revision).map(drop))?;
    stream.require_pending_peer()?;
    owner.stream=Some(stream);
    Ok(())
}

impl MoveEscrow{
    pub(in crate::smb::connection) fn send_interim(&mut self,owner:&mut Connection,input:&OriginalMoveInput)
        ->PeerResult<()>{
        if self.wire.debt.interim.is_some(){return Err(Fault::WrongReply);}
        let prepared=&self.prepared;
        let frame=self.wire.original(owner,input,&prepared.request,prepared.open)?;
        let [message]=frame.messages()else{return Err(Fault::WrongBatch);};
        let sid=prepared.request.sid;
        let rev=&self.wire.revision;
        let check=|c:&Connection|prepared.require_open(c,&VerifiedMember{message,ordinal:0,session:c.member_scope(sid,rev)?});
        check(owner)?;
        let plan=Interim::plan(owner.window,owner.in_flight,message)?;
        send(owner,&plan.bytes(message),sid,rev)?;
        check(owner)?;
        self.wire.debt.require(owner,message)?;
        // Publication is inseparable here through the original owner.
        // In particular: in_flight remains 1; the same debt still owes
        // a terminal and Drop still closes an abandoned original.
        self.wire.debt.interim=Some(plan.id);
        owner.window=plan.window;
        Ok(())
    }
}
impl WorkingMoveEscrow{
    fn require_original_open(&self,owner:&Connection,message:&Message<'_>,adopted:Option<&RustAdoptedMove>)->PeerResult<()>{
        let tree=self.request.root(owner)?;
        let slot=owner.open_table(self.request.sid)?.find(self.open)
            .filter(|slot|slot.tree()==self.request.tree).ok_or(Fault::SessionState)?;
        let Opened::File(file)=slot.object()else{return Err(Fault::SessionState);};
        let member=VerifiedMember{message,ordinal:0,session:owner.member_scope(self.request.sid,&self.wire.revision)?};
        let scope=member.rust_session_witness()?;
        match adopted{
            Some(adopted)=>self._guard.require_adopted(file,tree,&scope,adopted),
            None=>self._guard.require_original_open(file,tree,&scope),
        }.map_err(|_|Fault::SessionState)
    }

    /// Only the original worker's sealed pre-selection stop can use this
    /// terminal. Use the original SET_INFO, never respond to CANCEL.
    pub(in crate::smb::connection) fn send_worker_cancelled(&mut self,owner:&mut Connection,stopped:WorkerCancelled<'_>)
        ->PeerResult<()>{
        if self.final_started{return Err(Fault::NoReply);}
        let input=stopped.original_input();
        let frame=self.wire.original(owner,input,&self.request,self.open)?;
        let [message]=frame.messages()else{return Err(Fault::WrongBatch);};
        let id=self.wire.debt.interim.ok_or(Fault::NoReply)?;
        self.require_original_open(owner,message,None)?;
        let sid=self.request.sid;
        let rev=&self.wire.revision;
        let member=VerifiedMember{message,ordinal:0,session:owner.member_scope(sid,rev)?};
        let proof=CancelledOriginalMove{request:&self.request,open:self.open,async_id:Some(id)};
        let reply=UnsignedSmall::cancelled_move(&member,sid,&proof).map_err(|_|Fault::WrongReply)?;
        let (_,signer)=owner.valid_slot(sid).ok_or(Fault::SessionState)?;
        let signed=signer.sign_small_reply(rev,reply.credit(0)).map_err(|_|Fault::SessionState)?;
        self.final_started=true;
        send(owner,signed.bytes(),sid,rev)?;
        self.require_original_open(owner,message,None)?;
        self.wire.debt.require(owner,message)?;
        // Release only the original terminal debt, without granting a
        // second credit or rewinding the sequence range already advertised.
        owner.in_flight=owner.in_flight.checked_sub(1).ok_or(Fault::WrongBatch)?;
        self.wire.debt.original.owed=false;
        Ok(())
    }

    /// The peer already adopted this target and the original mover has
    /// rechecked the same physical witnesses. Even a full original write
    /// here does not clear the original debt or issue any credits.
    pub(in crate::smb::connection) fn write_confirmed(&mut self,owner:&mut Connection,input:&OriginalMoveInput,
        adopted:&RustAdoptedMove,before:CheckedMoveTarget)->PeerResult<MoveWritten>{
        if self.final_started||!before.matches(adopted.target()){return Err(Fault::WriteOutcomeUnknown);}
        let frame=self.wire.original(owner,input,&self.request,self.open)?;
        let [message]=frame.messages()else{return Err(Fault::WrongBatch);};
        let id=self.wire.debt.interim.ok_or(Fault::NoReply)?;
        self.require_original_open(owner,message,Some(adopted))?;
        let sid=self.request.sid;let rev=&self.wire.revision;
        let member=VerifiedMember{message,ordinal:0,session:owner.member_scope(sid,rev)?};
        let proof=ConfirmedOriginalMove{request:&self.request,open:self.open,async_id:id};
        let reply=UnsignedSmall::confirmed_move(&member,sid,&proof).map_err(|_|Fault::WrongReply)?;
        let (_,signer)=owner.valid_slot(sid).ok_or(Fault::SessionState)?;
        let signed=signer.sign_small_reply(rev,reply.credit(0)).map_err(|_|Fault::SessionState)?;
        self.final_started=true;
        send(owner,signed.bytes(),sid,rev)?;
        // Only the actual whole original stream write creates this ticket.
        Ok(MoveWritten(Arc::new(WriteSeal{input:self.wire.input.clone(),id,target:adopted.target().clone()})))
    }

    /// Revalidate the paid original and its same active OPEN after the
    /// mover's independent physical check of that signed transmission.
    /// This cannot send, grant credits, retry the copy or undo it.
    pub(in crate::smb::connection) fn settle_confirmed(&mut self,owner:&mut Connection,input:&OriginalMoveInput,
        adopted:&RustAdoptedMove,written:MoveWritten,request:CheckWrittenMove,after:CheckedMoveTarget)->PeerResult<()>{
        if !self.final_started||!Arc::ptr_eq(&written.0,&request.0)||!input.issued(&written.0.input)||
            self.wire.debt.interim!=Some(written.0.id)||!adopted.target().same(request.target())||
            !after.matches(request.target()){return Err(Fault::WriteOutcomeUnknown);}
        let frame=self.wire.original(owner,input,&self.request,self.open)?;
        let [message]=frame.messages()else{return Err(Fault::WrongBatch);};
        self.require_original_open(owner,message,Some(adopted))?;
        owner.stream.as_ref().ok_or(Fault::Transport)?.require_pending_peer()?;
        self.wire.debt.require(owner,message)?;
        owner.in_flight=owner.in_flight.checked_sub(1).ok_or(Fault::WrongBatch)?;
        self.wire.debt.original.owed=false;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/attached/smb/connection/sessions/ordinary/outbound/ordinary_move/async_turn/tests.rs"]
mod tests;
