//! Registration through selection, strict authentication and final submission.
//! The running server indexes unique IDs with weak currency markers; only
//! the original connection holds the mechanism phases and final session keys.
pub(in crate::smb) mod challenge;
pub(in crate::smb) mod proof;
pub(in crate::smb) mod finish;
pub(in crate::smb) mod ordinary;
mod clients;
pub(in crate::smb::connection) use clients::AcceptSeat;
use super::{Connection,Fault,Origin,Received,ReplySend,Result,setup::{Existing,Select}};
use crate::smb::{session::envelope::{AwaitNegotiate,HaveNegotiate},wire::setup_reply};
use std::{num::NonZeroU64,sync::{Arc,Mutex,OnceLock,Weak,atomic::Ordering},time::{Duration,Instant}};

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
    #[cfg(test)]
    pub(super) fn fixture_revoke_provider(&self) {
        self.run.verifier.as_ref().expect("the fixture holds a provider").fixture_revoke();
    }
    pub(in crate::smb) fn from_start(start:&Arc<crate::smb::InheritedRun>,verifier:crate::smb::session::ChallengeService)
        ->Result<Self>{
        let origin=verifier.batch_revision().and_then(|v|v.original_start(&verifier)).ok_or(Fault::NoVerifier)?;
        if !Arc::ptr_eq(start,&origin){return Err(Fault::NoVerifier);}
        let mut guid=[0u8;16];
        if !crate::smb::local::random(&mut guid)||guid==[0;16]{return Err(Fault::Entropy);}
        Ok(Self{run:Arc::new(Run::new(guid,Some(verifier)))})
    }
    pub(in crate::smb) fn is_original_start(&self,start:&Arc<crate::smb::InheritedRun>)->bool{
        self.run.verifier.as_ref().and_then(|s|s.batch_revision().and_then(|p|p.original_start(s)))
            .is_some_and(|loaded|Arc::ptr_eq(start,&loaded))
    }
}
pub(super) struct Run {
    guid:[u8;16],index:Mutex<Global>,verifier:Option<crate::smb::session::ChallengeService>,
    persistent:Arc<ordinary::opens::GlobalOpens>,
    share_modes:Arc<crate::smb::backend::OpenCoordinator>,
}
impl Run {
    fn new(guid:[u8;16],verifier:Option<crate::smb::session::ChallengeService>)->Self{
        Self{guid,index:Mutex::new(Global::empty()),verifier,persistent:ordinary::opens::GlobalOpens::new(),
            share_modes:crate::smb::backend::OpenCoordinator::new(ordinary::opens::PER_RUN)}
    }
}
struct Global {accepting:usize,next:u64,live:Vec<Weak<Claim>>,transports:Vec<clients::Tracked>,clients:Vec<clients::Dialect>}
impl Global {fn empty()->Self {Self{accepting:0,next:1,live:Vec::new(),transports:Vec::new(),clients:Vec::new()}}}
struct Claim { id:NonZeroU64, origin:Weak<Origin>, expires:Instant, valid:OnceLock<crate::smb::session::LiveMark> }
impl Claim {
    fn active(&self,now:Instant)->bool {
        self.origin.upgrade().is_some_and(|o|!o.closed.load(Ordering::Acquire) && match self.valid.get() {
            None=>now<self.expires,
            Some(mark)=>o.run.verifier.as_ref().is_some_and(|s|mark.current(s)),
        })
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
            let claim=Arc::new(Claim { id, origin:Arc::downgrade(origin),expires,valid:OnceLock::new() });
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
enum Phase { Sending, Waiting(AwaitNegotiate), Type1Taken, SendingChallenge, WaitingAuth(crate::smb::session::Pending), ProofTaken, SendingFinal, Valid(LiveSession) }
struct LiveSession {
    auth:crate::smb::session::BoundSession,opens:ordinary::opens::OpenTable,trees:ordinary::trees::TreeTable,
}
impl LiveSession {
    fn new(auth:crate::smb::session::BoundSession)->Self{
        Self{auth,opens:ordinary::opens::OpenTable::new(),trees:ordinary::trees::TreeTable::new()}
    }
}
#[derive(Clone,Copy)]
enum Sending {Selection,Challenge,Final}
impl Connection {
    fn sending(&self,claim:&Arc<Claim>,expected:Sending,now:Instant)->Result<()> {
        self.alive()?;
        let item=self.sessions.iter().find(|v|Arc::ptr_eq(&v.reg.claim,claim)).ok_or(Fault::SessionState)?;
        if claim.valid.get().is_some() || !matches!((expected,&item.phase),
            (Sending::Selection,Phase::Sending)|(Sending::Challenge,Phase::SendingChallenge)|
            (Sending::Final,Phase::SendingFinal)) || !item.reg.current(&self.origin,now)? {
            return Err(Fault::SessionState);
        }
        Ok(())
    }
    pub(in crate::smb::connection) fn has_protected_provider(&self)->bool {
        self.origin.run.verifier.as_ref().and_then(|s|s.batch_revision()).is_some()
    }
    /// Resource cleanup only; it conveys no session handle or store grant.
    pub(in crate::smb::connection) fn reap_and_has_valid(&mut self,now:Instant)->bool {
        self.sessions.retain(|item|item.reg.claim.active(now));
        self.sessions.iter().any(|item|matches!(&item.phase,Phase::Valid(_)))
    }
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
    pub(in crate::smb::connection) fn send(self)->Result<()> {
        let Self{tx,claim,next}=self;
        // Only offer this mechanism on the real transport while a protected
        // provider actually exists. Selection sends no challenge or identity.
        let stamp=tx.plan.owner.origin.run.verifier.as_ref().and_then(|s|s.batch_revision()).ok_or(Fault::NoVerifier)?;
        tx.send_setup((claim,next,stamp),|c,(claim,_,stamp),now|{
            c.sending(claim,Sending::Selection,now)?;
            let s=c.origin.run.verifier.as_ref().ok_or(Fault::NoVerifier)?;
            if !stamp.current(s) {return Err(Fault::SessionState);}Ok(())
        },|c,(claim,next,stamp),now|{
            let s=c.origin.run.verifier.as_ref().ok_or(Fault::NoVerifier)?;
            if !stamp.current(s) {return Err(Fault::SessionState);}
            Self::install(c,&claim,next,now)
        })
    }
    fn install(c:&mut Connection,claim:&Arc<Claim>,next:AwaitNegotiate,now:Instant)->Result<()> {
        c.sending(claim,Sending::Selection,now)?;
        let item=c.sessions.iter_mut().find(|x|Arc::ptr_eq(&x.reg.claim,claim)).ok_or(Fault::SessionState)?;
        item.phase=Phase::Waiting(next);Ok(())
    }
    #[cfg(test)]
    pub(in crate::smb) fn bytes(&self)->&[u8] {self.tx.bytes()}
    #[cfg(test)]
    pub(in crate::smb) fn submitted(self)->Result<()> {self.submitted_at(Instant::now())}
    #[cfg(test)]
    fn submitted_at(self,now:Instant)->Result<()> {
        let Self{tx,claim,next}=self;tx.publish_with(|c|Self::install(c,&claim,next,now))
    }
}

#[must_use]
pub(in crate::smb) enum Resume<'raw> {
    AnotherConnection(Existing<'raw>),
    NoSession(Declined<'raw>),
    EstablishedSetupUnsupported(Declined<'raw>), // no reauth/binding, never reset an existing live session
    InvalidToken(Declined<'raw>),
    ProofRejected(Declined<'raw>), // uniform strict mechanism error; never identify a password or principal
    Type1(ContinuedType1<'raw>),
    Proven(proof::Proven<'raw>), // mechanism result only; no success or usable session
    TransportGone,
}
pub(in crate::smb) struct Declined<'raw> { pub(in crate::smb::connection) original:Received<'raw> }
pub(in crate::smb) struct ContinuedType1<'raw> { pub(in crate::smb::connection) original:Received<'raw>, phase:Option<HaveNegotiate>, claim:Arc<Claim> }
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
        if matches!(&owner.sessions[i].phase,Phase::Valid(_)) {
            return Resume::EstablishedSetupUnsupported(Declined{original:self.original});
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
