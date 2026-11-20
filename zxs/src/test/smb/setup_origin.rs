//! Public synthetic transport/first-token fixtures only; no actual account,
//! challenge, successful authentication, socket, timer or storage operation.
use super::*;
use crate::smb::{connection::{Connection,Fault,Opening},wire::{
    direct::Frame,message::Compound,preauth::tests::synthetic_exchange,
}};

fn frame(p: &[u8]) -> Vec<u8> {
    let mut out=(p.len() as u32).to_be_bytes().to_vec();out.extend_from_slice(p);out
}
fn parsed(p: &[u8]) -> Compound<'_> {
    let (f,rest)=Frame::split(p).unwrap().unwrap();assert!(rest.is_empty());Compound::parse(f).unwrap()
}
fn connection() -> Connection {
    let [mut q,mut r,..]=synthetic_exchange();
    q[14..16].copy_from_slice(&4u16.to_le_bytes());r[14..16].copy_from_slice(&4u16.to_le_bytes());
    let p=frame(&q); Opening::received(&parsed(&p)).unwrap().bind(r).unwrap().submitted().unwrap()
}
fn body(token: &[u8], mid: u64, sid: u64) -> Vec<u8> {
    let [_,_,start,_,_]=synthetic_exchange();
    let mut p=start[..88].to_vec();
    p[24..32].copy_from_slice(&mid.to_le_bytes());p[40..48].copy_from_slice(&sid.to_le_bytes());
    p[76..78].copy_from_slice(&88u16.to_le_bytes());p[78..80].copy_from_slice(&(token.len() as u16).to_le_bytes());
    p.extend_from_slice(token);p
}
fn ntlm1() -> Vec<u8> {
    let mut p=vec![0;40];p[..8].copy_from_slice(b"NTLMSSP\0");
    p[8..12].copy_from_slice(&1u32.to_le_bytes());p[12..16].copy_from_slice(&0x2208_0211u32.to_le_bytes());p
}
// RFC4178 initial NTLM-only MechTypeList with NO optimistic token.
const PICK: &[u8]=&[0x60,0x1c,6,6,0x2b,6,1,5,5,2,0xa0,0x12,0x30,0x10,
                    0xa0,0x0e,0x30,0x0c,6,10,0x2b,6,1,4,1,0x82,0x37,2,2,10];

#[test]
fn first_token_stage_is_owned_with_its_same_frame_origin_and_no_generic_setup_reply() {
    let mut first=connection();let mut other=connection();
    let p=frame(&body(PICK,1,0));let c=parsed(&p);
    let mut paid=first.receive(&c).unwrap();
    assert!(matches!(other.plan(&mut paid),Err(Fault::WrongBatch)));
    assert!(matches!(first.plan(&mut paid),Err(Fault::SetupPhase)));
    let Start::Select(selected)=paid.start_setup() else { panic!("no supported mechanism selection") };
    assert!(std::ptr::eq(selected.original.frame,&c));
    assert!(std::sync::Arc::ptr_eq(&selected.original.debit.origin,&first.origin));
    assert!(!std::sync::Arc::ptr_eq(&selected.original.debit.origin,&other.origin));
    drop(selected); // unfinished ordinary debit kills only its first transport
    assert!(matches!(first.alive(),Err(Fault::Closed)));assert!(other.alive().is_ok());

    let p=frame(&body(&ntlm1(),1,0));let c=parsed(&p);
    let Start::Type1(next)=other.receive(&c).unwrap().start_setup() else { panic!("missing original Type1") };
    assert_eq!(next.original.frame.messages()[0].raw,&p[4..]);
}

#[test]
fn malformed_first_or_unimplemented_compound_does_not_dispatch_or_retry() {
    let mut c=connection();let p=frame(&body(b"malformed",1,0));let f=parsed(&p);
    let Start::Refused(bad)=c.receive(&f).unwrap().start_setup() else { panic!("malformed token") };
    assert!(matches!(bad.reason(),Why::Mechanism(_)));assert_eq!(bad.original.debit.entries[0].mid,1);
    assert!(matches!(c.receive(&f),Err(Fault::Range))); // same bytes do not regain their MID
    drop(bad);

    let mut c=connection();let mut first=body(PICK,1,0);
    let n=first.len().next_multiple_of(8);first.resize(n,0);first[20..24].copy_from_slice(&(n as u32).to_le_bytes());
    let mut echo=body(&[],2,0);echo[12..14].copy_from_slice(&13u16.to_le_bytes());
    first.extend(echo);let p=frame(&first);let f=parsed(&p);
    let Start::Refused(bad)=c.receive(&f).unwrap().start_setup() else { panic!("no setup compound") };
    assert_eq!(bad.reason(),Why::SetupCompound);assert_eq!(bad.original.debit.entries.len(),2);
}

#[test]
fn continuation_is_not_a_fresh_session_and_closed_owner_invalidates_live_borrows() {
    let mut c=connection();let p=frame(&body(&ntlm1(),1,42));let f=parsed(&p);
    let Start::Existing(waiting)=c.receive(&f).unwrap().start_setup() else { panic!("not an existing-session candidate") };
    assert_eq!(waiting.original.frame.messages()[0].header.session,42);drop(waiting);

    let mut c=connection();let p=frame(&body(PICK,1,0));let f=parsed(&p);
    let held=c.receive(&f).unwrap(); drop(c); // valid raw bytes and refs, but original owner gone
    assert!(matches!(held.start_setup(),Start::TransportGone));

    let mut c=connection();let mut ordinary=body(&[],1,0);
    ordinary[12..14].copy_from_slice(&13u16.to_le_bytes());let p=frame(&ordinary);let f=parsed(&p);
    let Start::Other(mut paid)=c.receive(&f).unwrap().start_setup() else { panic!("ordinary command lost") };
    assert_eq!(paid.payload(0),Some(crate::smb::connection::Payload::NoVariableRule));
    assert!(c.plan(&mut paid).is_ok()); // original ordinary accounting, not a login
}
