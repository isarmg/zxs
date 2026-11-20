//! Sole publisher of a freshly authenticated, signed, one-channel SMB
//! session. A successful local return is never a Tree or a Core FILE grant.
use super::{Claim,Local,Phase,Registration,proof::Proven};
use crate::smb::{connection::{Connection,Fault,Origin,ReplySend,Result},session::BoundSession};
use std::{sync::{Arc,Weak,atomic::Ordering},time::Instant};

#[must_use]
pub(in crate::smb) struct FinalSend<'c,'b> {
    tx:ReplySend<'c,'b,'static>,claim:Arc<Claim>,next:BoundSession,
}
impl Proven<'_> {
    pub(in crate::smb) fn prepare<'c,'b>(&'b mut self,owner:&'c mut Connection)->Result<FinalSend<'c,'b>> {
        owner.alive()?;
        if !Arc::ptr_eq(&owner.origin,&self.original.debit.origin) {return Err(Fault::WrongBatch);}
        if self.proof.is_none() || !self.original.debit.owed {return Err(Fault::SessionState);}
        let i=owner.sessions.iter().position(|v|Arc::ptr_eq(&v.reg.claim,&self.claim)).ok_or(Fault::SessionState)?;
        if !matches!(&owner.sessions[i].phase,Phase::ProofTaken) {return Err(Fault::SessionState);}
        // Take its original registration while the owner is exclusively held.
        // A terminal failure below retires it from both indexes, not another SID.
        let Local{reg,..}=owner.sessions.swap_remove(i);
        let now=Instant::now();
        if reg.claim.valid.get().is_some() || !reg.current(&owner.origin,now)? {return Err(Fault::SessionState);}
        let run=Arc::clone(&owner.origin.run);
        let service=run.verifier.as_ref().ok_or(Fault::NoVerifier)?;
        let proof=self.proof.take().ok_or(Fault::SessionState)?;
        if !proof.current(service) {return Err(Fault::SessionState);}
        owner.pin_client_dialect()?; // final GSS success checks the actual negotiated client/dialect
        let plan=owner.plan_debit(&mut self.original.debit)?;
        let grant=plan.credit_response(0).ok_or(Fault::NoReply)?;
        let sealed=proof.seal(service,grant).map_err(|_|Fault::SessionState)?;
        let (tx,next)=sealed.bind_to(|bytes|plan.bind_owned(bytes))?;
        let now=Instant::now();
        if !next.same_id(reg.claim.id) || !next.current(service) || !reg.current(&tx.plan.owner.origin,now)? {
            return Err(Fault::SessionState);
        }
        let claim=Arc::clone(&reg.claim);
        tx.plan.owner.sessions.push(Local{reg,phase:Phase::SendingFinal});Ok(FinalSend{tx,claim,next})
    }
}
impl FinalSend<'_,'_> {
    /// Only the originating transport may write this exact immutable SMB
    /// packet. Recheck before releasing bytes. An earlier borrow cannot prove
    /// that the packet was submitted, or that the revision remains current.
    pub(in crate::smb) fn bytes(&self)->Result<&[u8]> {
        let c=&*self.tx.plan.owner;c.alive()?;
        let service=c.origin.run.verifier.as_ref().ok_or(Fault::NoVerifier)?;
        let item=c.sessions.iter().find(|v|Arc::ptr_eq(&v.reg.claim,&self.claim)).ok_or(Fault::SessionState)?;
        if !matches!(&item.phase,Phase::SendingFinal) || !self.next.current(service) ||
            !item.reg.current(&c.origin,Instant::now())? {return Err(Fault::SessionState);}
        Ok(self.tx.bytes())
    }
    /// Transport contract: only after complete submission on the original
    /// transport. A partial or uncertain write drops the connection instead.
    pub(in crate::smb) fn submitted(self)->Result<()> {self.submitted_at(Instant::now())}
    fn submitted_at(self,now:Instant)->Result<()> {
        let Self{tx,claim,next}=self;
        tx.publish_with(|c|{
            let origin=Arc::clone(&c.origin);let guid=c.client.guid;let dialect=c.server.dialect;
            let item=c.sessions.iter_mut().find(|v|Arc::ptr_eq(&v.reg.claim,&claim)).ok_or(Fault::SessionState)?;
            if !matches!(&item.phase,Phase::SendingFinal) {return Err(Fault::SessionState);}
            let Local{reg,phase}=item;
            reg.publish(&origin,guid,dialect,now,next,|state|*phase=Phase::Valid(state))
        })
    }
}
impl Registration {
    fn publish(&self,origin:&Arc<Origin>,guid:[u8;16],dialect:u16,now:Instant,live:BoundSession,
        install:impl FnOnce(BoundSession))->Result<()> {
        if !Arc::ptr_eq(&self.run,&origin.run) || !Weak::ptr_eq(&self.claim.origin,&Arc::downgrade(origin)) ||
            !live.same_id(self.claim.id) {return Err(Fault::SessionState);}
        // Registry then revision is the only nested lock order. No network
        // operation, key derivation, token processing or storage under either.
        let global=self.run.index.lock().map_err(|_|Fault::SessionState)?;
        if !global.pinned(origin,guid,dialect) ||
            !global.live.iter().any(|w|Weak::ptr_eq(w,&Arc::downgrade(&self.claim))) {
            return Err(Fault::SessionState);
        }
        let service=origin.run.verifier.as_ref().ok_or(Fault::NoVerifier)?;
        let result=live.install_while_current(service,|state,stamp|{
            if now.max(Instant::now())>=self.claim.expires || origin.closed.load(Ordering::Acquire) {
                return Err(Fault::SessionState);
            }
            self.claim.valid.set(stamp).map_err(|_|Fault::SessionState)?;
            install(state);Ok(())
        }).ok_or(Fault::SessionState)?;
        drop(global);
        result
    }
}

#[cfg(test)]
#[path = "../../../test/smb/setup_finish.rs"]
mod tests;
