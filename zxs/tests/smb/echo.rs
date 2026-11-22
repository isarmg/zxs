//! Independently fixed request CMACs and complete signed responses. Same
//! published synthetic accounts/handshake as ordinary.rs. No sockets, login
//! to a service or real clients; the controlled sink only exists under cfg(test).
use super::*;
use crate::smb::connection::{
    sessions::{Phase,Run,challenge::tests::provider,finish::tests::publish_at,ordinary::Admission,tests::connected_as},
    setup::tests::{frame,parsed},transport::DirectStream,
};
use std::{io::{self,ErrorKind,Write},sync::{Arc,Mutex}};
const CASES:[(&str,&str);7]=[
    ("b1d07954d3eb2327da0e978dc0f1561f",concat!(
        "fe534d4240000100000000000d00020039000000000000000300000000000000",
        "473322110000000001000000000000000cb464681020097a630cef5860f1b3bd",
        "04000000")),
    ("fa57186dabad3909d57ee181978f67fd",concat!(
        "fe534d4240000100000000000d00020039000000000000000300000000000000",
        "47332211000000000100000000000000d4cdd66a1f36f56b1227895eb7539131",
        "04000000")),
    ("9f892d0f49dcd400e21d778012404178",concat!(
        "fe534d42400001000d0000c00d00020039000000000000000300000000000000",
        "473322110000000001000000000000009522300eb11d08ff6c2d5ee23f408eb8",
        "0900000000000000")),
    ("8fcdd7475138f5eb11091fda51084f52",concat!(
        "fe534d4240000100000000000d00020039000000480000000300000000000000",
        "47332211000000000100000000000000d97a08f020f5335819e7dc71681a93ae",
        "0400000000000000")),
    ("69ef64f45c2a03f5837970ac0cad7107",concat!(
        "fe534d4240000100000000000d0002003d000000000000000400000000000000",
        "48332211000000000100000000000000c449fdc2acff2f4251c72da32a7d1d9d",
        "0400000000000000")),
    ("7031d6d0f64114b41a173d133a792187",concat!(
        "fe534d4240000100000000000d00020039000000480000000500000000000000",
        "49332211000000000100000000000000a961c716535054a971491b8e388943b1",
        "0400000000000000")),
    ("a428c8f835efc65315daf9d0e7f469fc",concat!(
        "fe534d4240000100000000000d00020039000000000000000600000000000000",
        "4a33221100000000020000000000000013cd98319d57dedcc83b893a4b283a88",
        "0400000000000000")),
]; // single default/explicit, malformed, two related, two distinct sessions

fn hex(s:&str)->Vec<u8> {s.as_bytes().as_chunks::<2>().0.iter().map(|p|
    u8::from_str_radix(std::str::from_utf8(p).unwrap(),16).unwrap()).collect()}
fn packet(i:usize)->Vec<u8> {
    let (mid,sid,more,related):(u64,u64,bool,bool)=match i {
        0..=2=>(3,1,false,false),3=>(3,1,true,false),4=>(4,u64::MAX,false,true),
        5=>(5,1,true,false),6=>(6,2,false,false),_=>unreachable!(),
    };
    let mut p=vec![0;64];p[..4].copy_from_slice(b"\xfeSMB");p[4]=64;p[6]=1;p[12]=13;p[14]=2;
    p[16]=if related {0x3c} else {0x38};if more {p[20]=72;}
    p[24..32].copy_from_slice(&mid.to_le_bytes());p[32..36].copy_from_slice(&(0x11223344u32+mid as u32).to_le_bytes());
    if related {p[36..40].fill(0xff);}p[40..48].copy_from_slice(&sid.to_le_bytes());
    p[48..64].copy_from_slice(&hex(CASES[i].0));
    p.extend_from_slice(&[if i==2 {5} else {4},0,0xdc,0xba]); // invalid size vs deliberately ignored Reserved
    if matches!(i,3|5) {p.resize(72,0xe5);}p
}
#[derive(Default)]
struct Captured {bytes:Vec<u8>,calls:usize}
enum Mode {Normal,Fail,Zero,Revoke(Arc<Run>)}
struct Sink {seen:Arc<Mutex<Captured>>,mode:Mode}
impl io::Read for Sink {
    fn read(&mut self,_:&mut [u8])->io::Result<usize> {Err(ErrorKind::Unsupported.into())}
}
impl Write for Sink {
    fn write(&mut self,p:&[u8])->io::Result<usize> {
        let mut s=self.seen.lock().unwrap();s.calls+=1;
        if s.calls==1 {return Err(ErrorKind::Interrupted.into());}
        if s.calls==3 {match &self.mode {
            Mode::Fail=>return Err(ErrorKind::BrokenPipe.into()),
            Mode::Zero=>return Ok(0),
            Mode::Revoke(run)=>run.verifier.as_ref().unwrap().fixture_revoke(),
            Mode::Normal=>{},
        }}
        let n=p.len().min(11);s.bytes.extend_from_slice(&p[..n]);Ok(n)
    }
    fn flush(&mut self)->io::Result<()> {panic!("not buffered output or a storage FLUSH")}
}
fn attach_fixture(c:&mut Connection,mode:Mode)->Arc<Mutex<Captured>> {
    let seen=Arc::new(Mutex::new(Captured::default()));
    assert!(c.stream.is_none());
    c.stream=Some(DirectStream::Fixture(Box::new(Sink{seen:Arc::clone(&seen),mode})));seen
}
fn framed_cases(cases:&[usize],requests:bool)->Vec<u8> {
    let mut smb=Vec::new();
    for &i in cases {smb.extend(if requests {packet(i)} else {hex(CASES[i].1)});}
    frame(&smb)
}

#[test]
fn real_authenticated_echo_emits_exact_record_only_then_grants_under_both_negotiations() {
    for (cases,explicit) in [(&[0][..],false),(&[1][..],true),(&[2][..],false),(&[3,4][..],false),(&[5,6][..],false)] {
        let srv=provider();let mut c=connected_as(&srv,explicit).unwrap();
        assert_eq!(publish_at(&mut c,1),1);
        if cases[0]==5 {assert_eq!(publish_at(&mut c,3),2);}
        let sink=attach_fixture(&mut c,Mode::Normal);
        let input=framed_cases(cases,true);let actual=parsed(&input);
        let paid=c.receive(&actual).unwrap();
        assert_eq!(c.window.available(),4-cases.len() as u16);
        let Admission::Ready(mut batch)=paid.preflight(&mut c) else {panic!("must authenticate whole batch")};
        let out=batch.echoes().unwrap().unwrap();
        assert_eq!(out.tx.plan.owner.window.available(),4-cases.len() as u16);
        assert!(sink.lock().unwrap().bytes.is_empty());
        out.send().unwrap();
        assert!(matches!(batch.echoes(),Err(Fault::SessionState)));
        assert!(batch.next().is_err());drop(batch);
        assert_eq!(c.window.available(),4+cases.len() as u16);assert_eq!(c.in_flight,0);
        let sent=sink.lock().unwrap();assert!(sent.calls>2);
        assert_eq!(sent.bytes,framed_cases(cases,false));
        assert!(c.sessions.iter().all(|s|matches!(&s.phase,Phase::Valid(_))));
    }
}

#[test]
fn missing_output_revoke_or_uncertain_short_send_never_replenishes_or_releases_another_sender() {
    for kind in 0..5 {
        let srv=provider();let mut c=connected_as(&srv,false).unwrap();
        assert_eq!(publish_at(&mut c,1),1);
        let sink=match kind {
            0=>None,1=>Some(attach_fixture(&mut c,Mode::Normal)),2=>Some(attach_fixture(&mut c,Mode::Fail)),
            3=>Some(attach_fixture(&mut c,Mode::Zero)),_=>Some(attach_fixture(&mut c,Mode::Revoke(Arc::clone(&srv.run)))),
        };
        let input=framed_cases(&[0],true);let fr=parsed(&input);let paid=c.receive(&fr).unwrap();
        let Admission::Ready(mut batch)=paid.preflight(&mut c) else {panic!("whole preflight")};
        let out=batch.echoes().unwrap().unwrap();
        if kind==1 {srv.run.verifier.as_ref().unwrap().fixture_revoke();}
        assert!(out.send().is_err());drop(batch);
        assert!(matches!(c.alive(),Err(Fault::Closed)));assert!(c.sessions.is_empty());
        assert_eq!(c.window.available(),3);assert_eq!(c.in_flight,1);
        if let Some(s)=sink {
            let got=s.lock().unwrap();if kind==1 {assert!(got.bytes.is_empty());}
            else {let expect=framed_cases(&[0],false);assert!(!got.bytes.is_empty() && got.bytes.len()<expect.len());
                assert!(expect.starts_with(&got.bytes));}
        }
    }
}

#[test]
fn a_valid_tree_connect_is_not_answered_as_echo_or_consumed_by_this_handler() {
    let srv=provider();let mut c=connected_as(&srv,false).unwrap();assert_eq!(publish_at(&mut c,1),1);
    let capture=attach_fixture(&mut c,Mode::Normal);
    let mut q=hex("fe534d424000010000000000030001000800000000000000030000000000000000000000000000002a00000000000000000000000000000000000000000000000900000048001a005c005c0066006900780074007500720065005c005a0058005300");q[40..48].copy_from_slice(&1u64.to_le_bytes());
    q[48..64].copy_from_slice(&hex("701a0a44733d60ecbdcbf53ef90b7db6"));
    let input=frame(&q);let fr=parsed(&input);let paid=c.receive(&fr).unwrap();
    let Admission::Ready(mut batch)=paid.preflight(&mut c) else {panic!("signed tree request")};
    assert!(batch.echoes().unwrap().is_none());
    assert!(!batch.try_send_tree_refusal().unwrap());
    let Some(Step::Verified(t))=batch.next().unwrap() else {panic!("original unrelated handler lost its ticket")};
    assert_eq!(t.message().header.command,3);assert!(capture.lock().unwrap().bytes.is_empty());
    assert!(matches!(t.rust_session_witness(),Err(Fault::NoVerifier)));
}

#[test]
fn a_completely_written_record_still_does_not_publish_before_original_owner_commit() {
    for revoke in [false,true] {
        let srv=provider();
        let mut c=connected_as(&srv,false).unwrap();
        assert_eq!(publish_at(&mut c,1),1);
        let sink=attach_fixture(&mut c,Mode::Normal);
        let input=framed_cases(&[0],true);
        let original=parsed(&input);
        let debit=c.receive(&original).unwrap();
        let Admission::Ready(mut batch)=debit.preflight(&mut c) else {panic!("original preflight");};
        let written=batch.echoes().unwrap().unwrap().write_original().unwrap();
        assert_eq!(sink.lock().unwrap().bytes,framed_cases(&[0],false));
        assert_eq!(written.0.tx.plan.owner.window.available(),3);
        assert_eq!(written.0.tx.plan.owner.in_flight,1);
        if revoke {
            srv.run.verifier.as_ref().unwrap().fixture_revoke();
            assert!(matches!(written.publish_ledger(),Err(Fault::SessionState)));
        } else {
            drop(written); // uncertain local state after full socket write
        }
        drop(batch);
        assert!(matches!(c.alive(),Err(Fault::Closed)));
        assert!(c.sessions.is_empty());
        assert!(c.stream.is_none());
        assert_eq!(c.window.available(),3);
        assert_eq!(c.in_flight,1);
    }
}
