//! Synthetic, full raw/optimistic setup through the real ledger. Submitted
//! models the transport completion callback; no socket/credential operation.
use super::*;
use crate::smb::connection::{
    sessions::{Resume,LIFETIME,challenge::tests::{challenged,challenged_at,provider},tests::connected},
    setup::{Start,tests::{body,frame,parsed}},
};
use crate::smb::wire::message::Compound;
use std::time::Duration;

fn last_body(t:&[u8],sid:u64)->Vec<u8> {
    last_at(t,sid,2)
}
fn last_at(t:&[u8],sid:u64,mid:u64)->Vec<u8> {
    let mut p=body(t,mid,sid);p[16..20].copy_from_slice(&0x20u32.to_le_bytes());
    p[32..36].copy_from_slice(&0x44332211u32.to_le_bytes());frame(&p)
}
fn proven<'a>(f:&'a Compound<'a>,c:&mut Connection)->Proven<'a> {
    let Start::Existing(x)=c.receive(f).unwrap().start_setup() else {panic!("not final scoped request");};
    let Resume::Proven(proof)=x.continue_setup(c) else {panic!("not a strict mechanism result");};proof
}
// Fixture setup still performs the complete paid handshake and publishes its
// exact signed final through the original connection. No signer injection.
pub(in crate::smb::connection::sessions) fn publish_at(c:&mut Connection,first:u64)->u64 {
    let (sid,last)=challenged_at(c,false,first);let data=last_at(&last,sid,first+1);
    let fr=parsed(&data);let mut proof=proven(&fr,c);
    proof.prepare(c).unwrap().submitted().unwrap();sid
}
#[test]
fn submitted_exact_signed_final_installs_only_its_live_scope_and_a_fresh_credit() {
    for wrapped in [false,true] {
        let run=provider();let mut c=connected(&run);let mut other=connected(&run);
        let (sid,last)=challenged(&mut c,wrapped);let raw=last_body(&last,sid);let fr=parsed(&raw);
        let mut proof=proven(&fr,&mut c);let claim=Arc::clone(&proof.claim);
        assert!(matches!(proof.prepare(&mut other),Err(Fault::WrongBatch)));
        assert!(run.run.index.lock().unwrap().clients.is_empty());
        let send=proof.prepare(&mut c).unwrap();let bytes=send.bytes().unwrap();
        let tag=if wrapped {[0xec,0x2f,0xa0,0x21,0xcb,0x38,0x2a,0x7d,0x89,0xf8,0x16,0xe1,0xac,0xe1,0x20,0x90]}
            else {[0xb6,0x40,0x86,0xad,0xc8,0xa7,0xf9,0x75,0x1f,0xd8,0x7e,0xe6,0x82,0x1c,0xcc,0x10]};
        assert_eq!(&bytes[48..64],&tag[..]);
        assert_eq!(bytes.len(),if wrapped {101} else {72});
        assert_eq!(bytes[16],0x29);assert_eq!(&bytes[32..36],&[0x11,0x22,0x33,0x44]);
        assert!(matches!(&send.tx.plan.owner.sessions[0].phase,Phase::SendingFinal));
        assert!(claim.valid.get().is_none());assert!(other.sessions.is_empty());
        send.submitted().unwrap();assert_eq!(c.window.available(),4);
        let Phase::Valid(state)=&c.sessions[0].phase else {panic!("signed send did not promote");};
        assert!(state.same_id(claim.id));assert!(claim.valid.get().is_some());
        // NTLM gives no distinct session expiry in this local profile. The
        // initial 30 seconds were for the handshake, not a renewable grant.
        assert!(claim.active(claim.expires+Duration::from_secs(1)));
        let reauth=frame(&body(&last,3,sid));let fr=parsed(&reauth);
        let Start::Existing(x)=c.receive(&fr).unwrap().start_setup() else {panic!("missing existing SID")};
        let Resume::EstablishedSetupUnsupported(held)=x.continue_setup(&mut c) else {panic!("reauth must not replace valid session")};
        assert!(matches!(&c.sessions[0].phase,Phase::Valid(_)));
        run.run.verifier.as_ref().unwrap().fixture_revoke();assert!(!claim.active(Instant::now()));
        drop(held);drop(c);assert!(run.run.index.lock().unwrap().live.is_empty());
    }
}
#[test]
fn revoked_expired_and_abandoned_final_never_install_any_key_or_grant() {
    for kind in 0..3 {
        let run=provider();let mut c=connected(&run);let (sid,last)=challenged(&mut c,false);
        let raw=last_body(&last,sid);let fr=parsed(&raw);let mut proof=proven(&fr,&mut c);
        let claim=Arc::clone(&proof.claim);let send=proof.prepare(&mut c).unwrap();
        match kind {
            0=>{run.run.verifier.as_ref().unwrap().fixture_revoke();
                assert!(send.bytes().is_err());assert!(send.submitted().is_err());},
            1=>{assert!(send.submitted_at(claim.expires+LIFETIME).is_err());},
            _=>{drop(send);},
        }
        assert!(claim.valid.get().is_none());assert!(matches!(c.alive(),Err(Fault::Closed)));
        assert!(c.sessions.is_empty());assert!(run.run.index.lock().unwrap().live.is_empty());
    }
}
