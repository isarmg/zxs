//! Synthetic, fixed raw / SPNEGO mechanism proof via the actual paid request
//! and original pending slot. Reaching Proven is not a login or a tree grant.
use super::*;
use crate::smb::connection::{
    sessions::{challenge::tests::{challenged,provider},tests::connected},
    setup::{Start,tests::{body,frame,parsed}},
};

#[test]
fn only_original_paid_type3_consumes_registered_verifier_once() {
    for wrapped in [false,true] {
        let run=provider();let mut c=connected(&run);let mut other=connected(&run);
        let (sid,last)=challenged(&mut c,wrapped);let claim=Arc::clone(&c.sessions[0].reg.claim);
        let packet=frame(&body(&last,2,sid));let frame2=parsed(&packet);
        let Start::Existing(x)=c.receive(&frame2).unwrap().start_setup() else {panic!("not original SETUP")};
        let Resume::AnotherConnection(x)=x.continue_setup(&mut other) else {panic!("cross-connection use");};
        let Resume::Proven(verified)=x.continue_setup(&mut c) else {panic!("valid fixed NTLMv2 failed stage");};
        assert!(std::ptr::eq(verified.original.frame,&frame2));assert!(Arc::ptr_eq(&verified.claim,&claim));
        assert!(verified.proof.as_ref().unwrap().current(run.run.verifier.as_ref().unwrap()));
        assert!(matches!(&c.sessions[0].phase,Phase::ProofTaken));
        // Fresh MID but identical token cannot repeat the same challenge or
        // replace the valid terminal proof held by this original request.
        let next=frame(&body(&last,3,sid));let f=parsed(&next);
        let Start::Existing(x)=c.receive(&f).unwrap().start_setup() else {panic!("missing nonzero SID")};
        let Resume::NoSession(stale)=x.continue_setup(&mut c) else {panic!("reused pending proof")};
        assert!(matches!(&c.sessions[0].phase,Phase::ProofTaken));assert!(other.sessions.is_empty());
        drop(stale);drop(verified);
    }
}
#[test]
fn bad_terminal_proof_or_revoked_provider_consumes_only_that_sessions_slot() {
    for revoke in [false,true] {
        let run=provider();let mut c=connected(&run);let mut neighbor=connected(&run);
        let (sid,mut last)=challenged(&mut c,false);let (other,_) = challenged(&mut neighbor,false);
        if revoke {run.run.verifier.as_ref().unwrap().fixture_revoke();} else {last[72]^=1;}
        let pkt=frame(&body(&last,2,sid));let f=parsed(&pkt);
        let Start::Existing(x)=c.receive(&f).unwrap().start_setup() else {panic!("not terminal setup")};
        let Resume::ProofRejected(reject)=x.continue_setup(&mut c) else {panic!("failed proof advanced")};
        assert!(c.sessions.is_empty());assert_eq!(neighbor.sessions[0].reg.claim.id.get(),other);
        assert_eq!(run.run.index.lock().unwrap().live.len(),1);drop(reject);
    }
}

pub(in crate::smb) fn fixture_terminal(wrapped:bool,check:impl FnOnce(MechanismProof,&crate::smb::session::ChallengeService)) {
    let run=provider();let mut c=connected(&run);let (sid,last)=challenged(&mut c,wrapped);
    let mut original=body(&last,2,sid);original[16..20].copy_from_slice(&0x20u32.to_le_bytes());
    original[32..36].copy_from_slice(&0x44332211u32.to_le_bytes());
    let pkt=frame(&original);let f=parsed(&pkt);
    let Start::Existing(x)=c.receive(&f).unwrap().start_setup() else {panic!("not terminal SETUP")};
    let Resume::Proven(mut ctx)=x.continue_setup(&mut c) else {panic!("fixture failed actual proof");};
    check(ctx.proof.take().unwrap(),run.run.verifier.as_ref().unwrap());
}
