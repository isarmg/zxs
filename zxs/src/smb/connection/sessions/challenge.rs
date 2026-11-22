//! Move one exact paid Type 1 into a real unsigned MORE bound to this
//! transport, this run's protected service and this one absolute deadline.
use super::{Claim,ContinuedType1,Local,Phase,Sending};
use crate::smb::{
    connection::{Connection,Fault,Received,ReplySend,Result,setup::Type1},
    session::{ChallengeService,ChallengeStep,Pending,envelope::HaveNegotiate},
    wire::{self,setup_reply},
};
use std::{num::NonZeroU64,sync::Arc,time::Instant};

#[must_use]
pub(in crate::smb) struct ChallengeSend<'c,'b> {
    tx:ReplySend<'c,'b,'static>, claim:Arc<Claim>, next:Pending,
}
impl Type1<'_> {
    pub(in crate::smb) fn challenge<'c,'b>(&'b mut self,owner:&'c mut Connection)->Result<ChallengeSend<'c,'b>> {
        prepare(&mut self.original,&mut self.phase,None,owner,Instant::now(),ChallengeService::issue)
    }
}
impl ContinuedType1<'_> {
    pub(in crate::smb) fn challenge<'c,'b>(&'b mut self,owner:&'c mut Connection)->Result<ChallengeSend<'c,'b>> {
        prepare(&mut self.original,&mut self.phase,Some(&self.claim),owner,Instant::now(),ChallengeService::issue)
    }
}
fn prepare<'c,'b>(original:&'b mut Received<'_>,phase:&mut Option<HaveNegotiate>,prior:Option<&Arc<Claim>>,
    owner:&'c mut Connection,now:Instant,
    issue:impl FnOnce(&ChallengeService,HaveNegotiate,NonZeroU64,Instant)
        ->std::result::Result<ChallengeStep,crate::smb::session::ChallengeRejected>)->Result<ChallengeSend<'c,'b>> {
    owner.alive()?;
    if !Arc::ptr_eq(&owner.origin,&original.debit.origin) {return Err(Fault::WrongBatch);}
    if phase.is_none() || !original.debit.owed {return Err(Fault::SessionState);}
    let run=Arc::clone(&owner.origin.run);
    let service=run.verifier.as_ref().ok_or(Fault::NoVerifier)?;
    let [m]=original.frame.messages() else {return Err(Fault::SetupPhase);};
    let reg=if let Some(prior)=prior {
        let i=owner.sessions.iter().position(|v|Arc::ptr_eq(&v.reg.claim,prior)).ok_or(Fault::SessionState)?;
        if m.header.session!=prior.id.get() || !matches!(&owner.sessions[i].phase,Phase::Type1Taken) {
            return Err(Fault::SessionState);
        }
        let valid=owner.sessions[i].reg.current(&owner.origin,now)?;
        // This chain consumes the original registration, even on later
        // failure. Never reserve a second SID or restart its deadline.
        let Local{reg,..}=owner.sessions.swap_remove(i);
        if !valid {return Err(Fault::SessionState);} reg
    } else {
        if m.header.session!=0 {return Err(Fault::SessionState);}
        owner.reserve_pending(now)?
    };
    let claim=Arc::clone(&reg.claim);
    let plan=owner.plan_debit(&mut original.debit)?;
    let grant=plan.credit_response(0).ok_or(Fault::NoReply)?;
    let start=phase.take().ok_or(Fault::SessionState)?;
    let step=issue(service,start,claim.id,claim.expires).map_err(|_|Fault::SetupPhase)?;
    let actual=setup_reply::more(m,claim.id.get(),grant,step.token()).map_err(|e|match e {
        wire::Error::Limit=>Fault::Resources,_=>Fault::SetupPhase })?;
    let next=step.bind(&actual).map_err(|_|Fault::SetupPhase)?;
    let checked=Instant::now();
    if !next.current_at(service,checked) || !reg.current(&plan.owner.origin,checked)? {return Err(Fault::SessionState);}
    let tx=plan.bind_owned(actual)?;
    // Fresh admission reserved room; a continued admission just removed its
    // original slot while holding this connection exclusively.
    tx.plan.owner.sessions.push(Local{reg,phase:Phase::SendingChallenge});
    Ok(ChallengeSend{tx,claim,next})
}
impl ChallengeSend<'_,'_> {
    pub(in crate::smb::connection) fn send(self)->Result<()> {
        let Self{tx,claim,next}=self;
        tx.send_setup((claim,next),|c,(claim,next),now|{
            c.sending(claim,Sending::Challenge,now)?;
            let service=c.origin.run.verifier.as_ref().ok_or(Fault::NoVerifier)?;
            if !next.current_at(service,now) {return Err(Fault::SessionState);}Ok(())
        },|c,(claim,next),now|Self::install(c,&claim,next,now))
    }
    fn install(c:&mut Connection,claim:&Arc<Claim>,next:Pending,now:Instant)->Result<()> {
        c.sending(claim,Sending::Challenge,now)?;
        let origin=Arc::clone(&c.origin);
        let service=origin.run.verifier.as_ref().ok_or(Fault::NoVerifier)?;
        let item=c.sessions.iter_mut().find(|x|Arc::ptr_eq(&x.reg.claim,claim)).ok_or(Fault::SessionState)?;
        next.install_while_current(service,now,|pending|{
            item.phase=Phase::WaitingAuth(pending);
        }).ok_or(Fault::SessionState)
    }
}

#[cfg(test)]
#[path = "../../../../tests/smb/setup_challenge.rs"]
pub(in crate::smb::connection) mod tests;

#[cfg(test)]
#[path = "../../../../tests/smb/setup_send.rs"]
mod send_tests;

#[cfg(test)]
include!("../../../../tests/unit/attached/smb/connection/sessions/challenge/items.rs");
