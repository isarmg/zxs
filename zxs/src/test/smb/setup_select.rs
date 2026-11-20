//! Synthetic, bounded server/session scopes only. Nothing creates a production
//! server, runs a socket, reads a credential or grants access to any file.
use super::*;
use crate::smb::connection::{Opening,OpeningSend,setup::{Start,tests::{PICK,body,frame,ntlm1,parsed}}};
use crate::smb::wire::preauth::tests::synthetic_exchange;

pub(in crate::smb::connection) fn running(guid:[u8;16])->RunningServerGuid {
    RunningServerGuid {run:Arc::new(Run{guid,index:Mutex::new(Global{next:1,live:Vec::new()}),verifier:None})}
}
impl Opening {
    // For the existing ledger-only fixtures. Its known server wire bytes are
    // a0..af. Do not derive an owner from a packet, even in this test bridge.
    pub(in crate::smb::connection) fn fixture_bind(self,actual:Vec<u8>)->Result<OpeningSend> {
        self.bind(actual,&running(std::array::from_fn(|i|0xa0+i as u8)))
    }
}
pub(super) fn connected(run:&RunningServerGuid)->Connection {
    let [mut q,mut r,..]=synthetic_exchange();r[72..88].copy_from_slice(run.wire());
    q[14..16].copy_from_slice(&4u16.to_le_bytes());r[14..16].copy_from_slice(&4u16.to_le_bytes());
    let raw=frame(&q);Opening::received(&parsed(&raw)).unwrap().bind(r,run).unwrap().submitted().unwrap()
}
pub(super) fn selected(c:&mut Connection,mid:u64,now:Instant)->Result<u64> {
    let bytes=frame(&body(PICK,mid,0));let f=parsed(&bytes);
    let Start::Select(mut phase)=c.receive(&f)?.start_setup() else {panic!("expected omitted optimistic token")};
    let s=phase.prepare_at(c,now)?;
    let id=u64::from_le_bytes(s.bytes()[40..48].try_into().unwrap());
    s.submitted_at(now)?;Ok(id)
}
fn continuing()->Vec<u8> {
    let mut token=vec![0xa1,46,0x30,44,0xa2,42,4,40];token.extend(ntlm1());token
}

#[test]
fn first_more_uses_original_request_credit_and_exact_request_mic_before_continuation() {
    let run=running([7;16]);let mut c=connected(&run);let mut wrong=connected(&run);let now=Instant::now();
    let mut q=body(PICK,1,0);q[16..20].copy_from_slice(&0x20u32.to_le_bytes());q[32..36].copy_from_slice(&[3,4,5,6]);
    let bytes=frame(&q);let f=parsed(&bytes);
    let Start::Select(mut phase)=c.receive(&f).unwrap().start_setup() else {panic!("not the selection path")};
    assert!(matches!(phase.prepare_at(&mut wrong,now),Err(Fault::WrongBatch)));
    assert!(wrong.sessions.is_empty());assert_eq!(run.run.index.lock().unwrap().next,1);
    let out=phase.prepare_at(&mut c,now).unwrap();
    // Full sync header, STATUS_MORE_PROCESSING_REQUIRED / MID 1 / SID 1 / credit 1;
    // 9/0/72/23 and exact RFC 4178 selected NTLM OID with request-mic (no Type 2).
    let mut expected=q[..64].to_vec();expected[8..12].copy_from_slice(&0xc0000016u32.to_le_bytes());
    expected[16..20].copy_from_slice(&0x21u32.to_le_bytes());expected[40..48].copy_from_slice(&1u64.to_le_bytes());
    expected.extend([9,0,0,0,72,0,23,0,0xa1,21,0x30,19,0xa0,3,10,1,3,0xa1,12,6,10,0x2b,6,1,4,1,0x82,0x37,2,2,10]);
    assert_eq!(out.bytes(),expected.as_slice());
    assert!(matches!(&out.tx.plan.owner.sessions[0].phase,Phase::Sending));
    assert_eq!(run.run.index.lock().unwrap().live.len(),1);
    out.submitted_at(now).unwrap();assert!(matches!(&c.sessions[0].phase,Phase::Waiting(_)));
    assert_eq!(c.window.available(),4);
    assert!(matches!(phase.prepare_at(&mut c,now),Err(Fault::SessionState)));
    let bytes=frame(&body(&continuing(),2,1));let f=parsed(&bytes);
    let Start::Existing(existing)=c.receive(&f).unwrap().start_setup() else {panic!("missing continued SID")};
    let Resume::AnotherConnection(existing)=existing.continue_at(&mut wrong,now) else {panic!("cross-connection use")};
    let Resume::Type1(bound)=existing.continue_at(&mut c,now) else {panic!("live original did not resume")};
    assert_eq!(bound.claim.id.get(),1);
    assert!(Arc::ptr_eq(&bound.claim,&c.sessions[0].reg.claim));
    assert!(std::ptr::eq(bound.original.frame,&f));assert!(matches!(&c.sessions[0].phase,Phase::Type1Taken));
}

#[test]
fn same_run_other_connection_cannot_take_or_cancel_known_sid_or_reuse_retired_id() {
    let run=running([9;16]);let now=Instant::now();let mut a=connected(&run);let mut b=connected(&run);
    assert_eq!(selected(&mut a,1,now).unwrap(),1);
    assert_eq!(selected(&mut b,1,now).unwrap(),2);
    let raw=frame(&body(&continuing(),2,1));let f=parsed(&raw);
    let Start::Existing(x)=b.receive(&f).unwrap().start_setup() else {panic!("not a continuation")};
    let Resume::NoSession(missing)=x.continue_at(&mut b,now) else {panic!("foreign SID became active")};
    assert_eq!(missing.original.frame.messages()[0].header.session,1);
    assert_eq!(a.sessions.len(),1);assert_eq!(b.sessions.len(),1);
    let invalid=frame(&body(b"not a DER response",2,1));let fi=parsed(&invalid);
    let Start::Existing(x)=a.receive(&fi).unwrap().start_setup() else {panic!("not the charged request")};
    let Resume::InvalidToken(denied)=x.continue_at(&mut a,now) else {panic!("malformed GSS was not rejected")};
    assert!(a.sessions.is_empty());assert_eq!(b.sessions.len(),1);
    assert_eq!(run.run.index.lock().unwrap().live.len(),1);
    // Keep paid error receipts alive: no actual network error writer is supplied here.
    assert_eq!(selected(&mut a,3,now).unwrap(),3);
    drop(denied);drop(missing);
}

#[test]
fn ambiguous_or_late_send_cannot_publish_continuation_or_credits() {
    let run=running([5;16]);let now=Instant::now();let mut c=connected(&run);
    let bytes=frame(&body(PICK,1,0));let f=parsed(&bytes);
    let Start::Select(mut phase)=c.receive(&f).unwrap().start_setup() else {panic!("not selection")};
    let send=phase.prepare_at(&mut c,now).unwrap();drop(send);
    assert!(matches!(c.alive(),Err(Fault::Closed)));assert!(c.sessions.is_empty());
    assert!(run.run.index.lock().unwrap().live.is_empty());
    let mut c=connected(&run);let Start::Select(mut phase)=c.receive(&f).unwrap().start_setup() else {panic!("not selection")};
    let send=phase.prepare_at(&mut c,now).unwrap();assert!(matches!(send.submitted_at(now+LIFETIME),Err(Fault::SessionState)));
    assert!(matches!(c.alive(),Err(Fault::Closed)));assert!(run.run.index.lock().unwrap().live.is_empty());
    let mut c=connected(&run);let sid=selected(&mut c,1,now).unwrap();
    let raw=frame(&body(&continuing(),2,sid));let f=parsed(&raw);
    let Start::Existing(x)=c.receive(&f).unwrap().start_setup() else {panic!("not waiting")};
    assert!(matches!(x.continue_at(&mut c,now+LIFETIME),Resume::NoSession(_)));
    assert!(c.sessions.is_empty());assert!(run.run.index.lock().unwrap().live.is_empty());
}

#[test]
fn bounded_run_and_transport_tables_expiry_cleanup_and_sentinel_refusal() {
    let run=running([11;16]);let now=Instant::now();let mut conns=Vec::new();
    for _ in 0..(PER_RUN/PER_CONNECTION) {
        let mut c=connected(&run);for n in 1..=PER_CONNECTION {selected(&mut c,n as u64,now).unwrap();}
        conns.push(c);
    }
    assert_eq!(run.run.index.lock().unwrap().live.len(),PER_RUN);
    let mut overflow=connected(&run);assert!(matches!(selected(&mut overflow,1,now),Err(Fault::SessionLimit)));
    assert!(overflow.sessions.is_empty());
    assert!(matches!(selected(&mut conns[0],PER_CONNECTION as u64+1,now),Err(Fault::SessionLimit)));
    let mut new=connected(&run);assert_eq!(selected(&mut new,1,now+LIFETIME).unwrap(),PER_RUN as u64+1);
    assert_eq!(run.run.index.lock().unwrap().live.len(),1); // expired weak/global registrations removed
    let end=running([12;16]);end.run.index.lock().unwrap().next=u64::MAX-1;
    let mut c=connected(&end);assert_eq!(selected(&mut c,1,now).unwrap(),u64::MAX-1);
    assert!(matches!(selected(&mut c,2,now),Err(Fault::Exhausted)));
    assert_eq!(end.run.index.lock().unwrap().next,u64::MAX);
}
