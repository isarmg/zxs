//! In-progress registration for SPNEGO selection and strict NTLM Type 2. The
//! running server owns the globally unique IDs; the originating transport
//! alone owns each protocol phase. Neither table has authenticated sessions.
pub(in crate::smb) mod challenge;
pub(in crate::smb) mod proof;
use super::{Connection,Fault,Origin,Received,ReplySend,Result,setup::{Existing,Select}};
use crate::smb::{session::envelope::{AwaitNegotiate,HaveNegotiate},wire::setup_reply};
use std::{num::NonZeroU64,sync::{Arc,Mutex,Weak,atomic::Ordering},time::{Duration,Instant}};

const PER_CONNECTION: usize=8;
const PER_RUN: usize=64;
const LIFETIME: Duration=Duration::from_secs(30);

/// Nonpublic construction: genuine service startup must generate a new GUID
/// and one registry, shared by all its transports. Knowing the 16 public wire
/// bytes cannot recreate this owner. Never borrow the old installation ID.
pub(in crate::smb) struct RunningServerGuid { run:Arc<Run> }
impl RunningServerGuid {
    pub(super) fn wire(&self)->&[u8;16] { &self.run.guid }
    pub(super) fn share(&self)->Arc<Run> { Arc::clone(&self.run) }
}
pub(super) struct Run { guid:[u8;16], index:Mutex<Global>, verifier:Option<crate::smb::session::ChallengeService> }
struct Global { next:u64, live:Vec<Weak<Claim>> }
struct Claim { id:NonZeroU64, origin:Weak<Origin>, expires:Instant }
impl Claim {
    fn active(&self,now:Instant)->bool {
        now<self.expires && self.origin.upgrade().is_some_and(|o|!o.closed.load(Ordering::Acquire))
    }
}
struct Registration { run:Arc<Run>, claim:Arc<Claim> }
impl Registration {
    fn reserve(origin:&Arc<Origin>,now:Instant)->Result<Self> {
        let run=Arc::clone(&origin.run);
        let expires=now.checked_add(LIFETIME).ok_or(Fault::Clock)?;
        let claim={
            let mut table=run.index.lock().map_err(|_|Fault::SessionState)?;
            table.live.retain(|w|w.upgrade().is_some_and(|s|s.active(now)));
            if table.live.len()>=PER_RUN {return Err(Fault::SessionLimit);}
            let id=NonZeroU64::new(table.next).filter(|n|n.get()!=u64::MAX).ok_or(Fault::Exhausted)?;
            table.live.try_reserve(1).map_err(|_|Fault::Resources)?;
            let claim=Arc::new(Claim { id, origin:Arc::downgrade(origin),expires });
            table.next+=1; // checked strictly below u64::MAX; IDs never recycled within this run
            table.live.push(Arc::downgrade(&claim));
            claim
        };
        Ok(Self{run,claim})
    }
    fn current(&self,origin:&Arc<Origin>,now:Instant)->Result<bool> {
        if !self.claim.active(now) || !Weak::ptr_eq(&self.claim.origin,&Arc::downgrade(origin)) ||
            !Arc::ptr_eq(&self.run,&origin.run) {return Ok(false);}
        let table=self.run.index.lock().map_err(|_|Fault::SessionState)?;
        Ok(table.live.iter().any(|w|w.upgrade().is_some_and(|s|s.id==self.claim.id && Arc::ptr_eq(&s,&self.claim))))
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut t)=self.run.index.lock() {
            t.live.retain(|w|!Weak::ptr_eq(w,&Arc::downgrade(&self.claim)));
        }
    }
}

pub(super) struct Local { reg:Registration, phase:Phase }
enum Phase { Sending, Waiting(AwaitNegotiate), Type1Taken, SendingChallenge, WaitingAuth(crate::smb::session::Pending), ProofTaken }
impl Connection {
    fn reserve_pending(&mut self,now:Instant)->Result<Registration> {
        self.alive()?;
        self.sessions.retain(|x|x.reg.claim.active(now));
        if self.sessions.len()>=PER_CONNECTION {return Err(Fault::SessionLimit);}
        self.sessions.try_reserve(1).map_err(|_|Fault::Resources)?;
        Registration::reserve(&self.origin,now)
    }
}

#[must_use]
pub(in crate::smb) struct SelectionSend<'c,'b> {
    tx:ReplySend<'c,'b,'static>, claim:Arc<Claim>, next:AwaitNegotiate,
}
impl Select<'_> {
    /// Reserve on THIS connection and its own run. No API accepts a wire SID,
    /// a chosen grant, a detached token or a replacement response buffer.
    pub(in crate::smb) fn prepare<'c,'b>(&'b mut self,owner:&'c mut Connection)->Result<SelectionSend<'c,'b>> {
        self.prepare_at(owner,Instant::now())
    }
    fn prepare_at<'c,'b>(&'b mut self,owner:&'c mut Connection,now:Instant)->Result<SelectionSend<'c,'b>> {
        owner.alive()?;
        if !Arc::ptr_eq(&owner.origin,&self.original.debit.origin) {return Err(Fault::WrongBatch);}
        if self.phase.is_none() || !self.original.debit.owed {return Err(Fault::SessionState);}
        let reg=owner.reserve_pending(now)?;
        let claim=Arc::clone(&reg.claim);
        let plan=owner.plan_debit(&mut self.original.debit)?;
        let [m]=self.original.frame.messages() else {return Err(Fault::SetupPhase);};
        let grant=plan.credit_response(0).ok_or(Fault::NoReply)?;
        let actual=setup_reply::select_more(m,claim.id.get(),grant).map_err(|e| match e {
            crate::smb::wire::Error::Limit=>Fault::Resources,_=>Fault::SetupPhase})?;
        let next=self.phase.take().ok_or(Fault::SessionState)?.selected(&actual).map_err(|_|Fault::SetupPhase)?;
        let tx=plan.bind_owned(actual)?;
        // Capacity already reserved. Both indexes exist before the borrow
        // releases bytes to the original sender; no continuation is usable yet.
        tx.plan.owner.sessions.push(Local{reg,phase:Phase::Sending});
        Ok(SelectionSend{tx,claim,next})
    }
}
impl SelectionSend<'_,'_> {
    pub(in crate::smb) fn bytes(&self)->&[u8] {self.tx.bytes()}
    /// Only after submitting those exact bytes to the original connection.
    pub(in crate::smb) fn submitted(self)->Result<()> {self.submitted_at(Instant::now())}
    fn submitted_at(self,now:Instant)->Result<()> {
        let Self{tx,claim,next}=self;
        tx.publish_with(|c|{
            let origin=Arc::clone(&c.origin);
            let item=c.sessions.iter_mut().find(|x|Arc::ptr_eq(&x.reg.claim,&claim)).ok_or(Fault::SessionState)?;
            if !matches!(&item.phase,Phase::Sending) || !item.reg.current(&origin,now)? {return Err(Fault::SessionState);}
            item.phase=Phase::Waiting(next); Ok(())
        })
    }
}

#[must_use]
pub(in crate::smb) enum Resume<'raw> {
    AnotherConnection(Existing<'raw>),
    NoSession(Declined<'raw>),
    InvalidToken(Declined<'raw>),
    Type1(ContinuedType1<'raw>),
    Proven(proof::Proven<'raw>), // mechanism result only; no success or usable session
    TransportGone,
}
pub(in crate::smb) struct Declined<'raw> { original:Received<'raw> }
pub(in crate::smb) struct ContinuedType1<'raw> { original:Received<'raw>, phase:Option<HaveNegotiate>, claim:Arc<Claim> }
impl<'raw> Existing<'raw> {
    /// Existing only means the packet carried a nonzero SID. Never allocate
    /// or reconstruct a session or challenge from a client's number.
    pub(in crate::smb) fn continue_setup(self,owner:&mut Connection)->Resume<'raw> {
        self.continue_at(owner,Instant::now())
    }
    fn continue_at(self,owner:&mut Connection,now:Instant)->Resume<'raw> {
        if !Arc::ptr_eq(&owner.origin,&self.original.debit.origin) {return Resume::AnotherConnection(self);}
        if owner.alive().is_err() || !self.original.debit.owed {return Resume::TransportGone;}
        let [m]=self.original.frame.messages() else {return Resume::NoSession(Declined{original:self.original});};
        let sid=m.header.session;
        // The server-global index is scoped back to the same original
        // transport. A wrong connection/SID never evicts someone else's slot.
        let Some(i)=owner.sessions.iter().position(|v|v.reg.claim.id.get()==sid) else {
            return Resume::NoSession(Declined{original:self.original});
        };
        match owner.sessions[i].reg.current(&owner.origin,now) {
            Ok(true)=>{},
            Ok(false)=>{owner.sessions.swap_remove(i);return Resume::NoSession(Declined{original:self.original});},
            Err(_)=>{owner.origin.closed.store(true,Ordering::Release);owner.sessions.clear();return Resume::TransportGone;},
        }
        if matches!(&owner.sessions[i].phase,Phase::WaitingAuth(_)) {
            return proof::received(self.original,owner,i);
        }
        if !matches!(&owner.sessions[i].phase,Phase::Waiting(_)) {
            return Resume::NoSession(Declined{original:self.original});
        }
        let Phase::Waiting(next)=std::mem::replace(&mut owner.sessions[i].phase,Phase::Type1Taken) else {unreachable!()};
        let result=if setup_reply::plain_header(m) { next.received(self.original.frame) }
            else { Err(crate::smb::session::envelope::Failure::Stage) };
        if owner.alive().is_err() {owner.sessions.clear();return Resume::TransportGone;}
        match result {
            Ok(phase)=>Resume::Type1(ContinuedType1 { original:self.original,phase:Some(phase),claim:Arc::clone(&owner.sessions[i].reg.claim) }),
            Err(_)=>{owner.sessions.swap_remove(i);Resume::InvalidToken(Declined{original:self.original})}
        }
    }
}

#[cfg(test)]
#[path = "../../test/smb/setup_select.rs"]
pub(in crate::smb::connection) mod tests;
