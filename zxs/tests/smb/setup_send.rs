//! Only public synthetic account/token material and an original in-memory
//! duplex. These tests never connect, open a port or inject an authenticated
//! principal. Existing strict/KDF fixtures independently fix the wire tags.
use super::*;
use crate::smb::connection::{
    sessions::{Resume,Run,challenge::tests::provider,tests::connected},
    setup::{Start,tests::{body,frame,parsed}},transport::DirectStream,
};
use std::{io::{self,ErrorKind,Read,Write},sync::Mutex};

enum Action {Break(usize),Revoke(usize,Arc<Run>)}
#[derive(Default)]
struct Capture {out:Vec<u8>,calls:usize,action:Option<Action>,drops:usize}
struct Stream(Arc<Mutex<Capture>>);
impl Read for Stream {fn read(&mut self,_:&mut [u8])->io::Result<usize> {Err(ErrorKind::Unsupported.into())}}
impl Write for Stream {
    fn write(&mut self,src:&[u8])->io::Result<usize> {
        let mut c=self.0.lock().unwrap();c.calls+=1;if c.calls%7==1 {return Err(ErrorKind::Interrupted.into());}
        let limit=match &c.action {None=>usize::MAX,Some(Action::Break(n)|Action::Revoke(n,_))=>*n};
        let count=src.len().min(7).min(limit.saturating_sub(c.out.len()));
        if count==0 {return Err(ErrorKind::BrokenPipe.into());}
        c.out.extend_from_slice(&src[..count]);
        if matches!(&c.action,Some(Action::Revoke(n,_)) if *n==c.out.len()) &&
            let Some(Action::Revoke(_,run))=c.action.take() {run.verifier.as_ref().unwrap().fixture_revoke();}
        Ok(count)
    }
    fn flush(&mut self)->io::Result<()> {panic!("not a buffered file")}
}
impl Drop for Stream {fn drop(&mut self) {self.0.lock().unwrap().drops+=1;}}
fn attached(server:&super::super::RunningServerGuid)->(Connection,Arc<Mutex<Capture>>) {
    let mut c=connected(server);let v=Arc::new(Mutex::new(Capture::default()));
    c.stream=Some(DirectStream::Fixture(Box::new(Stream(Arc::clone(&v)))));(c,v)
}
fn actual(f:&Arc<Mutex<Capture>>)->Vec<u8> {f.lock().unwrap().out.clone()}
// Sources are previous exact signed-final KATs, not derived by the sender.
const RAW:[u8;16]=[0xb6,0x40,0x86,0xad,0xc8,0xa7,0xf9,0x75,0x1f,0xd8,0x7e,0xe6,0x82,0x1c,0xcc,0x10];
const WRAPPED:[u8;16]=[0xec,0x2f,0xa0,0x21,0xcb,0x38,0x2a,0x7d,0x89,0xf8,0x16,0xe1,0xac,0xe1,0x20,0x90];
fn last_packet(token:&[u8],mid:u64)->Vec<u8> {
    let mut p=body(token,mid,1);p[16..20].copy_from_slice(&0x20u32.to_le_bytes());
    p[32..36].copy_from_slice(&0x44332211u32.to_le_bytes());frame(&p)
}
#[test]
fn selection_type2_and_signed_final_transition_only_on_the_same_complete_framed_stream() {
    for style in 0..3 {
        let server=provider();let (mut c,seen)=attached(&server);let mut expected=Vec::new();
        let (first,term)=ChallengeService::fixture_messages(style==1);
        let (pick,later,last)=ChallengeService::fixture_selected_messages();
        let chain=style==2;
        if chain {
            let wire=frame(&body(&pick,1,0));let fr=parsed(&wire);
            let Start::Select(mut request)=c.receive(&fr).unwrap().start_setup() else {panic!("selection")};
            let tx=request.prepare(&mut c).unwrap();expected.extend(frame(tx.bytes()));
            assert!(actual(&seen).is_empty());tx.send().unwrap();assert_eq!(actual(&seen),expected);
            assert!(matches!(c.sessions[0].phase,Phase::Waiting(_)));
        }
        {
            let raw=if chain {frame(&body(&later,2,1))} else {frame(&body(&first,1,0))};
            let fr=parsed(&raw);let paid=c.receive(&fr).unwrap();
            // No public nonce injection: deterministic source is cfg(test).
            let mut direct;let mut cont;
            let tx=match paid.start_setup() {
                Start::Type1(x)=>{direct=x;prepare(&mut direct.original,&mut direct.phase,None,&mut c,Instant::now(),ChallengeService::fixture_issue).unwrap()},
                Start::Existing(x)=>{let Resume::Type1(x)=x.continue_setup(&mut c) else {panic!("expected actual continuation")};
                    cont=x;prepare(&mut cont.original,&mut cont.phase,Some(&cont.claim),&mut c,Instant::now(),ChallengeService::fixture_issue).unwrap()},
                _=>panic!("expected Type 1"),
            };
            assert_eq!(&tx.bytes()[8..12],&0xc0000016u32.to_le_bytes());
            assert_eq!(actual(&seen),expected);expected.extend(frame(tx.bytes()));tx.send().unwrap();
            assert_eq!(actual(&seen),expected);assert!(matches!(c.sessions[0].phase,Phase::WaitingAuth(_)));
            assert_eq!(c.window.available(),4);
        }
        let raw=if chain {last_packet(&last,3)} else {last_packet(&term,2)};let fr=parsed(&raw);
        let Start::Existing(x)=c.receive(&fr).unwrap().start_setup() else {panic!("paid Type 3")};
        let Resume::Proven(mut proof)=x.continue_setup(&mut c) else {panic!("strict original proof")};
        let claim=Arc::clone(&proof.claim);let tx=proof.prepare(&mut c).unwrap();
        if !chain {assert_eq!(&tx.bytes().unwrap()[48..64],if style==0 {&RAW[..]} else {&WRAPPED[..]});}
        assert!(claim.valid.get().is_none());assert_eq!(actual(&seen),expected);
        expected.extend(frame(tx.bytes().unwrap()));tx.send().unwrap();
        assert_eq!(actual(&seen),expected);assert_eq!(c.window.available(),4);assert_eq!(c.in_flight,0);
        assert!(claim.valid.get().is_some());assert!(matches!(c.sessions[0].phase,Phase::Valid(_)));
    }
}

#[test]
fn partial_or_revoked_at_the_last_byte_cannot_settle_either_more_or_final() {
    for stage in 0..3 {for revoke in [false,true] {
        let server=provider();let (mut c,seen)=attached(&server);
        let (one,three)=ChallengeService::fixture_messages(false);
        let (pick,_,_)=ChallengeService::fixture_selected_messages();
        let arm=|n:usize|{
            let mut s=seen.lock().unwrap();let at=s.out.len()+if revoke {4+n} else {2};
            s.action=Some(if revoke {Action::Revoke(at,server.share())} else {Action::Break(at)});
        };
        if stage==0 {
            let wire=frame(&body(&pick,1,0));let fr=parsed(&wire);
            let Start::Select(mut q)=c.receive(&fr).unwrap().start_setup() else {panic!("selection")};
            let tx=q.prepare(&mut c).unwrap();arm(tx.bytes().len());assert!(tx.send().is_err());
        } else {
            let wire=frame(&body(&one,1,0));let fr=parsed(&wire);
            let Start::Type1(mut q)=c.receive(&fr).unwrap().start_setup() else {panic!("Type 1")};
            let tx=prepare(&mut q.original,&mut q.phase,None,&mut c,Instant::now(),ChallengeService::fixture_issue).unwrap();
            if stage==1 {arm(tx.bytes().len());assert!(tx.send().is_err());}
            else {
                tx.send().unwrap();
                let raw=last_packet(&three,2);let fr=parsed(&raw);
                let Start::Existing(x)=c.receive(&fr).unwrap().start_setup() else {panic!("Type 3")};
                let Resume::Proven(mut proof)=x.continue_setup(&mut c) else {panic!("typed proof")};
                let claim=Arc::clone(&proof.claim);let tx=proof.prepare(&mut c).unwrap();
                arm(tx.bytes().unwrap().len());assert!(tx.send().is_err());assert!(claim.valid.get().is_none());
            }
        }
        assert!(matches!(c.alive(),Err(Fault::Closed)));assert!(c.sessions.is_empty() && c.stream.is_none());
        assert_eq!(c.window.available(),3);assert_eq!(c.in_flight,1);assert_eq!(seen.lock().unwrap().drops,1);
    }}
}
