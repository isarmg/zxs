//! First ordinary synchronous response on the original transport. Only an
//! all-ECHO, fully authenticated batch can use this sender. No file receipts
//! or generic (status, bytes) shortcut.
use super::{CheckedBatch,Step,VerifiedMember};
use crate::smb::{
    connection::{Connection,Fault,ReplySend,Result},
    session::{BatchRevision,VerifiedIds},
    wire::{echo_reply::{MAX_ECHO_BYTES,UnsignedEcho},message::RELATED},
};

#[must_use]
pub(in crate::smb) struct EchoSend<'a> {
    tx:ReplySend<'a,'a,'static>,ids:&'a VerifiedIds,revision:&'a BatchRevision,count:usize,
}
fn current(c:&Connection,ids:&VerifiedIds,revision:&BatchRevision,count:usize)->Result<()> {
    c.alive()?;
    for i in 0..count {let id=ids.at(i).ok_or(Fault::SessionState)?;c.member_scope(id,revision)?;}
    Ok(())
}
impl VerifiedMember<'_,'_> {
    /// For the ECHO codec only. Expose no numeric session ID or signer; even
    /// a related request's original spelling cannot switch its response key.
    pub(in crate::smb) fn is_echo_session(&self,id:std::num::NonZeroU64)->bool {
        self.message().header.command==13 && self.session().reg.claim.id==id
    }
}
impl CheckedBatch<'_,'_> {
    /// Ok(None) means another *unconsumed* command batch still belongs to
    /// its dispatcher. Never turn a TREE/OPEN request into an ECHO success.
    pub(in crate::smb) fn echoes(&mut self)->Result<Option<EchoSend<'_>>> {
        if self.next!=0 || self.halted || !self.original.debit.owed {return Err(Fault::SessionState);}
        if self.original.frame.messages().iter().any(|m|m.header.command!=13) {return Ok(None);}
        let count=self.original.frame.messages().len();let compound=count>1;
        let mut drafts=Vec::new();drafts.try_reserve_exact(count).map_err(|_|Fault::Resources)?;
        let mut previous=None;
        for i in 0..count {
            let Some(Step::Verified(member))=self.next()? else {return Err(Fault::SessionState);};
            member.session().recheck()?;
            let id=member.session().reg.claim.id;let m=member.message();
            if m.header.flags&RELATED!=0 && previous!=Some(id) {return Err(Fault::SessionState);}
            drafts.push(UnsignedEcho::encode(&member,id,compound,i+1<count).map_err(|_|Fault::WrongReply)?);
            // Each actual ECHO has resolved this live Session. Even a parameter
            // error creates no Tree/Open; the following ECHO needs neither.
            previous=Some(id);
        }
        self.halted=true; // prevent replaying the semantic result or settling twice
        let ids=&self.ids;let revision=&self.revision;
        let plan=self.owner.plan_debit(&mut self.original.debit)?;
        let mut record=Vec::new();
        record.try_reserve_exact(4+count*MAX_ECHO_BYTES).map_err(|_|Fault::Resources)?;
        record.extend_from_slice(&[0;4]);
        for (i,draft) in drafts.into_iter().enumerate() {
            let grant=plan.credit_response(i).ok_or(Fault::NoReply)?;
            let payable=draft.credit(grant);let sid=payable.sid();
            if ids.at(i)!=Some(sid) {return Err(Fault::SessionState);}
            plan.owner.member_scope(sid,revision)?;
            let (_,bound)=plan.owner.valid_slot(sid).ok_or(Fault::SessionState)?;
            let signed=bound.sign_echo(revision,payable).map_err(|_|Fault::SessionState)?;
            record.extend_from_slice(signed.bytes());
        }
        let n=record.len()-4;
        if n as u64>u64::from(plan.owner.limits.transact)+256 {return Err(Fault::FrameSize);}
        record[..4].copy_from_slice(&(n as u32).to_be_bytes());
        let tx=plan.bind_record(record)?;
        Ok(Some(EchoSend{tx,ids,revision,count}))
    }
}
impl EchoSend<'_> {
    /// No caller-supplied writer, bytes or manual "submitted" acknowledgement.
    /// The whole immutable Direct TCP record must be accepted, in order, by
    /// the source connection's owned stream. This is NOT peer delivery/ACK.
    pub(in crate::smb) fn send(self)->Result<()> {
        let Self{tx,ids,revision,count}=self;
        if tx.start!=4 {return Err(Fault::WrongReply);}
        current(tx.plan.owner,ids,revision,count)?;
        let mut out=tx.plan.owner.stream.take().ok_or(Fault::Transport)?;
        let c:&Connection=&*tx.plan.owner;
        out.write_record(tx.actual.as_ref(),||current(c,ids,revision,count))?;
        tx.plan.owner.stream=Some(out);
        // If the original scope has died, even a submitted record cannot leave
        // a reusable transport with a contradictory credit window.
        tx.publish_with(|c|current(c,ids,revision,count))
    }
}

#[cfg(test)]
#[path = "../../../../test/smb/echo.rs"]
mod tests;
