//! Same original registered challenge -> strictly verified mechanism proof.
//! This stage cannot construct a session from a SID or from a user's name.
use super::{Claim,Declined,Phase,Resume};
use crate::smb::{
    connection::{Connection,Received}, session::MechanismProof, wire::setup_reply,
};
use std::{sync::{Arc,atomic::Ordering},time::Instant};

#[must_use]
pub(in crate::smb) struct Proven<'raw> {
    pub(in crate::smb::connection) original:Received<'raw>, pub(super) proof:Option<MechanismProof>, pub(super) claim:Arc<Claim>,
}
pub(super) fn received<'raw>(original:Received<'raw>,owner:&mut Connection,i:usize)->Resume<'raw> {
    // Caller has located and checked this very registration using the paid
    // request's effective SID on its original live transport. Removing the
    // one-shot verifier from the table precedes all token processing.
    let Phase::WaitingAuth(pending)=std::mem::replace(&mut owner.sessions[i].phase,Phase::ProofTaken)
        else {unreachable!()};
    let run=Arc::clone(&owner.origin.run);
    let result=run.verifier.as_ref().and_then(|service|{
        let [m]=original.frame.messages() else {return None;};
        if !setup_reply::plain_header(m) {return None;}
        pending.prove_current(service,original.frame).ok()
    });
    if owner.alive().is_err() {owner.sessions.clear();return Resume::TransportGone;}
    let checked=owner.sessions[i].reg.current(&owner.origin,Instant::now());
    if checked.is_err() {
        owner.origin.closed.store(true,Ordering::Release);owner.sessions.clear();return Resume::TransportGone;
    }
    if checked==Ok(true) && let Some(proof)=result &&
        run.verifier.as_ref().is_some_and(|s|proof.current(s)) {
        return Resume::Proven(Proven{original,proof:Some(proof),claim:Arc::clone(&owner.sessions[i].reg.claim)});
    }
    owner.sessions.swap_remove(i);
    Resume::ProofRejected(Declined{original})
}

#[cfg(test)]
#[path = "../../../../tests/smb/setup_proof.rs"]
pub(in crate::smb) mod tests;
