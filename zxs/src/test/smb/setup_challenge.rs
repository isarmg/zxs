//! Whole originating request -> exact challenge MORE -> original Pending. No
//! production subject, final success, network activity, or file authority.
use super::*;
use crate::smb::connection::{
    sessions::{Global,Run,RunningServerGuid,Resume,LIFETIME,tests::{connected,running,selected}},
    setup::{Start,tests::{PICK,body,frame,ntlm1,parsed}},
};
use std::sync::Mutex;

pub(in crate::smb::connection::sessions) fn provider()->RunningServerGuid {
    RunningServerGuid{run:Arc::new(Run{guid:[17;16],index:Mutex::new(Global::empty()),
        verifier:Some(ChallengeService::fixture())})}
}
fn one()->Vec<u8> {
    let mut p=ntlm1();p[12..16].copy_from_slice(&0x2208_8215u32.to_le_bytes());
    p[32..].copy_from_slice(&[10,0,0x65,0x4a,0,0,0,15]);p
}
fn initial(one:&[u8])->Vec<u8> {
    let mut out=PICK.to_vec();for i in [1,11,13] {out[i]+=44;}
    out.extend_from_slice(&[0xa2,42,4,40]);out.extend_from_slice(one);out
}
fn continued(one:&[u8])->Vec<u8> {
    let mut t=vec![0xa1,46,0x30,44,0xa2,42,4,40];t.extend_from_slice(one);t
}

#[test]
fn fresh_raw_and_optimistic_wait_for_the_exact_sent_type2_never_authenticate() {
    let now=Instant::now();let srv=provider();
    for wrapped in [false,true] {
        let mut c=connected(&srv);let mut neighbor=connected(&srv);
        let raw=one();let gss=if wrapped {initial(&raw)} else {raw};
        let mut req=body(&gss,1,0);req[16..20].copy_from_slice(&0x40u32.to_le_bytes());
        let data=frame(&req);let f=parsed(&data);
        let Start::Type1(mut x)=c.receive(&f).unwrap().start_setup() else {panic!("not an actual Type 1");};
        assert!(matches!(x.challenge(&mut neighbor),Err(Fault::WrongBatch)));
        let s=prepare(&mut x.original,&mut x.phase,None,&mut c,now,ChallengeService::fixture_issue).unwrap();
        let id=s.claim.id;assert_eq!(s.claim.expires,now+LIFETIME);
        assert_eq!(&s.bytes()[8..12],&0xc0000016u32.to_le_bytes());
        assert_eq!(&s.bytes()[16..20],&0x41u32.to_le_bytes());
        assert_eq!(&s.bytes()[24..32],&1u64.to_le_bytes());
        assert_eq!(&s.bytes()[40..48],&id.get().to_le_bytes());
        assert_eq!(&s.bytes()[48..70],&[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,9,0,0,0,72,0]);
        let n=u16::from_le_bytes(s.bytes()[70..72].try_into().unwrap()) as usize;assert_eq!(s.bytes().len(),72+n);
        if wrapped {
            assert_eq!(s.bytes()[72],0xa1);
            assert!(s.bytes()[72..].windows(5).any(|v|v==[0xa0,3,10,1,3])); // first request-mic
        } else {
            assert_eq!(&s.bytes()[72..80],b"NTLMSSP\0");assert_eq!(&s.bytes()[96..104],&[0x11,0x12,0x13,0x14,0x15,0x16,0x17,0x18]);
        }
        assert!(matches!(&s.tx.plan.owner.sessions[0].phase,Phase::SendingChallenge));
        s.submitted_at(now).unwrap();
        assert!(matches!(&c.sessions[0].phase,Phase::WaitingAuth(_)));
        assert_eq!(c.sessions.len(),1);assert_eq!(c.window.available(),4);assert!(neighbor.sessions.is_empty());
        assert!(matches!(x.challenge(&mut c),Err(Fault::SessionState)));
    }
}

#[test]
fn previous_selection_uses_original_claim_deadline_and_only_the_next_token() {
    let run=provider();let now=Instant::now();let mut c=connected(&run);
    let id=selected(&mut c,1,now).unwrap();let old=Arc::clone(&c.sessions[0].reg.claim);
    let packet=frame(&body(&continued(&one()),2,id));let f=parsed(&packet);
    let Start::Existing(x)=c.receive(&f).unwrap().start_setup() else {panic!("not an existing setup");};
    let Resume::Type1(mut bound)=x.continue_setup(&mut c) else {panic!("lost original selection");};
    let s=prepare(&mut bound.original,&mut bound.phase,Some(&bound.claim),&mut c,now,ChallengeService::fixture_issue).unwrap();
    assert!(Arc::ptr_eq(&s.claim,&old));assert_eq!(s.claim.expires,now+LIFETIME);
    assert_eq!(&s.bytes()[24..32],&2u64.to_le_bytes());assert_eq!(&s.bytes()[40..48],&id.to_le_bytes());
    let gss=&s.bytes()[72..];assert!(gss.windows(5).any(|v|v==[0xa0,3,10,1,1]));
    assert!(!gss.windows(6).any(|v|v==[0xa1,12,6,10,0x2b,6])); // no repeated supportedMech
    assert_eq!(run.run.index.lock().unwrap().next,2);
    assert!(matches!(s.submitted_at(now+LIFETIME),Err(Fault::SessionState)));
    assert!(matches!(c.alive(),Err(Fault::Closed)));
    assert!(run.run.index.lock().unwrap().live.is_empty());
}

#[test]
fn no_provider_no_challenge_and_changed_revision_or_uncertain_send_never_publishes() {
    let data=frame(&body(&one(),1,0));let f=parsed(&data);
    let empty=running([3;16]);let mut c=connected(&empty);
    let Start::Type1(mut x)=c.receive(&f).unwrap().start_setup() else {panic!("not Type 1")};
    assert!(matches!(x.challenge(&mut c),Err(Fault::NoVerifier)));assert!(c.sessions.is_empty());
    assert_eq!(empty.run.index.lock().unwrap().next,1);
    for revoke in [false,true] {
        let srv=provider();let mut c=connected(&srv);
        let Start::Type1(mut x)=c.receive(&f).unwrap().start_setup() else {panic!("not Type 1")};
        let s=prepare(&mut x.original,&mut x.phase,None,&mut c,Instant::now(),ChallengeService::fixture_issue).unwrap();
        if revoke {
            srv.run.verifier.as_ref().unwrap().fixture_revoke();
            assert!(matches!(s.submitted(),Err(Fault::SessionState)));
        } else {drop(s);}
        assert!(matches!(c.alive(),Err(Fault::Closed)));assert!(c.sessions.is_empty());
        assert!(srv.run.index.lock().unwrap().live.is_empty());
    }
}

pub(in crate::smb::connection::sessions) fn challenged(c:&mut Connection,wrapped:bool)->(u64,Vec<u8>) {
    let (one,last)=ChallengeService::fixture_messages(wrapped);
    let packet=frame(&body(&one,1,0));let f=parsed(&packet);
    let Start::Type1(mut x)=c.receive(&f).unwrap().start_setup() else {panic!("fixture must start with actual Type 1")};
    let send=prepare(&mut x.original,&mut x.phase,None,c,Instant::now(),ChallengeService::fixture_issue).unwrap();
    let sid=send.claim.id.get();send.submitted().unwrap();(sid,last)
}
