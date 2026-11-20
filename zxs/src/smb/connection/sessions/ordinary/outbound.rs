//! Original synchronous stream and ledger for closed reply kinds. There is
//! no generic signing/send route from caller-provided bytes or success status.
use super::{CheckedBatch,Step,VerifiedMember};
use crate::smb::{
    backend::tree_connect::{Error as TreeError,TreeConnectRequest},
    connection::{Connection,Fault,Payload,ReplySend,Result},
    session::{BatchRevision,VerifiedIds},
    wire::{fixed_reply::{MAX_SMALL_REPLY,TreeFailure,UnsignedSmall},
        message::{RELATED,Routing},path_fields::FieldError},
};
use std::num::NonZeroU64;

#[must_use]
pub(in crate::smb) struct SyncReply<'a> {
    tx:ReplySend<'a,'a,'static>,ids:&'a VerifiedIds,revision:&'a BatchRevision,count:usize,
}
fn current(c:&Connection,ids:&VerifiedIds,revision:&BatchRevision,count:usize)->Result<()> {
    c.alive()?;
    for i in 0..count {let id=ids.at(i).ok_or(Fault::SessionState)?;c.member_scope(id,revision)?;}
    Ok(())
}
impl VerifiedMember<'_,'_> {
    /// Used by the closed reply codec. No numeric ID or signer leaves
    /// the original session; a related spelling cannot select a key.
    pub(in crate::smb) fn response_session_matches(&self,id:NonZeroU64)->bool {
        self.session().reg.claim.id==id
    }
}
impl CheckedBatch<'_,'_> {
    fn before_reply(&self)->Result<()> {
        if self.next!=0 || self.halted || !self.original.debit.owed {return Err(Fault::SessionState);}
        self.owner.alive()
    }
    /// Ok(None) leaves a non-ECHO frame untouched for its actual dispatcher.
    pub(in crate::smb) fn echoes(&mut self)->Result<Option<SyncReply<'_>>> {
        self.before_reply()?;
        if self.original.frame.messages().iter().any(|m|m.header.command!=13) {return Ok(None);}
        let count=self.original.frame.messages().len();let compound=count>1;
        let mut drafts=Vec::new();drafts.try_reserve_exact(count).map_err(|_|Fault::Resources)?;
        let mut previous=None;
        for i in 0..count {
            let Some(Step::Verified(member))=self.next()? else {return Err(Fault::SessionState);};
            member.session().recheck()?;
            let id=member.session().reg.claim.id;let m=member.message();
            if m.header.flags&RELATED!=0 && previous!=Some(id) {return Err(Fault::SessionState);}
            drafts.push(UnsignedSmall::echo(&member,id,compound,i+1<count).map_err(|_|Fault::WrongReply)?);
            // A parameter error creates no Tree/Open. The following ECHO needs neither.
            previous=Some(id);
        }
        self.small_replies(drafts.into_iter()).map(Some)
    }

    /// Finish only the plain standalone TCON failures that can be known
    /// without a Core/alias issuer. An acceptable ZXS path or a transport/
    /// extension shape without a disposition keeps its exact paid member.
    /// No numeric ID, independently supplied error or caller body is accepted.
    pub(in crate::smb) fn try_send_tree_refusal(&mut self)->Result<bool> {
        self.before_reply()?;
        let [m]=self.original.frame.messages() else {return Ok(false);};
        if m.header.command!=3 || m.header.routing!=(Routing::Sync{tree:0}) {return Ok(false);}
        if self.original.payload(0)!=Some(Payload::NoVariableRule) {return Err(Fault::SessionState);}
        let id=self.ids.at(0).ok_or(Fault::SessionState)?;
        let member=VerifiedMember{message:m,ordinal:0,session:self.owner.member_scope(id,&self.revision)?};
        // This checks syntax only. It cannot authorize a matching share,
        // which still depends on its trusted listener and real Core scope.
        let (member,why)=match TreeConnectRequest::parse(member) {
            Ok(_)=>return Ok(false),
            Err((_,TreeError::Field(FieldError::Unsupported)))=>return Ok(false),
            Err((_,TreeError::Session(e)))=>return Err(e),
            Err((m,TreeError::BadUnc|TreeError::Field(FieldError::Malformed)))=>(m,TreeFailure::InvalidParameter),
            Err((m,TreeError::UnknownShare))=>(m,TreeFailure::BadNetworkName),
            Err((m,TreeError::Resources))=>(m,TreeFailure::InsufficientResources),
            Err((_,TreeError::Field(_)))=>return Err(Fault::WrongReply),
        };
        let draft=UnsignedSmall::tree_failure(&member,id,why).map_err(|_|Fault::WrongReply)?;
        self.next=1;
        self.small_replies(std::iter::once(draft))?.send()?;
        Ok(true)
    }

    // No generic reply builder outside this module. Only the reply kinds
    // above create drafts; their bytes and members cannot be interchanged.
    fn small_replies(&mut self,drafts:impl ExactSizeIterator<Item=UnsignedSmall>)->Result<SyncReply<'_>> {
        let count=drafts.len();
        if count==0 || self.next!=count || count!=self.original.frame.messages().len() {
            return Err(Fault::WrongReply);
        }
        self.halted=true;
        let ids=&self.ids;let revision=&self.revision;
        let plan=self.owner.plan_debit(&mut self.original.debit)?;
        let mut record=Vec::new();
        record.try_reserve_exact(4+count*MAX_SMALL_REPLY).map_err(|_|Fault::Resources)?;
        record.extend_from_slice(&[0;4]);
        for (i,draft) in drafts.enumerate() {
            let grant=plan.credit_response(i).ok_or(Fault::NoReply)?;
            let payable=draft.credit(grant);let sid=payable.sid();
            if ids.at(i)!=Some(sid) {return Err(Fault::SessionState);}
            plan.owner.member_scope(sid,revision)?;
            let (_,bound)=plan.owner.valid_slot(sid).ok_or(Fault::SessionState)?;
            let signed=bound.sign_small_reply(revision,payable).map_err(|_|Fault::SessionState)?;
            record.extend_from_slice(signed.bytes());
        }
        let n=record.len()-4;
        if n as u64>u64::from(plan.owner.limits.transact)+256 {return Err(Fault::FrameSize);}
        record[..4].copy_from_slice(&(n as u32).to_be_bytes());
        let tx=plan.bind_record(record)?;
        Ok(SyncReply{tx,ids,revision,count})
    }
}
/// Inaccessible to dispatch callers. This value means only that the exact
/// bound bytes were completely written on their own still-current stream.
/// It owns the unresolved ledger; dropping it closes this connection.
#[must_use]
struct FullyWrittenSync<'a>(SyncReply<'a>);

impl<'a> SyncReply<'a> {
    /// No caller-supplied sink, bytes or completion bit. A complete transport
    /// write is followed by a separate actual state/credit publication.
    pub(in crate::smb) fn send(self)->Result<()> {
        self.write_original()?.publish_ledger()
    }

    fn write_original(self)->Result<FullyWrittenSync<'a>> {
        let Self{tx,ids,revision,count}=self;
        if tx.start!=4 {return Err(Fault::WrongReply);}
        current(tx.plan.owner,ids,revision,count)?;
        let mut out=tx.plan.owner.stream.take().ok_or(Fault::Transport)?;
        let c:&Connection=&*tx.plan.owner;
        out.write_record(tx.actual.as_ref(),||current(c,ids,revision,count))?;
        // Also check after the last successful write, not just before it.
        current(c,ids,revision,count)?;
        tx.plan.owner.stream=Some(out);
        Ok(FullyWrittenSync(Self{tx,ids,revision,count}))
    }
}

impl FullyWrittenSync<'_> {
    fn publish_ledger(self)->Result<()> {
        let SyncReply{tx,ids,revision,count}=self.0;
        // Already written bytes cannot be recalled. A last loss of the
        // original session only closes this connection without new credits.
        tx.publish_with(|c|current(c,ids,revision,count))
    }
}

#[cfg(test)]
#[path = "../../../../test/smb/echo.rs"]
mod tests;
