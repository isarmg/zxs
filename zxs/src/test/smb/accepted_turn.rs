//! No listener or accounts. A single synthetic full-duplex stream carries
//! negotiation through the final strict packet and exactly one signed ECHO.
//! The test-only challenge is public fixed data; the salt/time in the real
//! NEGOTIATE serializer are not predicted. Compare against the bytes written.
use super::*;
use crate::smb::{connection::{sessions::{RunningServerGuid,challenge::tests::provider,tests::running},
    setup::tests::{body,frame},transport::DirectStream},session::ChallengeService,
    wire::preauth::tests::synthetic_exchange};
use aes::Aes128;
use cmac::Cmac;
use hmac::{Hmac,KeyInit,Mac};
use sha2::{Digest,Sha256,Sha512};
use std::{collections::VecDeque,io::{self,ErrorKind,Read,Write},net::IpAddr,sync::{Arc,Mutex}};

#[derive(Default)]
struct Trace {input:VecDeque<u8>,out:Vec<u8>,eof:bool,drops:usize}
struct Duplex(Arc<Mutex<Trace>>);
impl Read for Duplex {
    fn read(&mut self,b:&mut [u8])->io::Result<usize>{
        let mut t=self.0.lock().unwrap();
        if t.input.is_empty() {return if t.eof {Ok(0)} else {Err(ErrorKind::WouldBlock.into())};}
        let n=b.len().min(9).min(t.input.len());
        for x in &mut b[..n] {*x=t.input.pop_front().unwrap();}Ok(n)
    }
}
impl Write for Duplex {
    fn write(&mut self,b:&[u8])->io::Result<usize>{let n=b.len().min(13);
        self.0.lock().unwrap().out.extend_from_slice(&b[..n]);Ok(n)}
    fn flush(&mut self)->io::Result<()> {panic!("not buffered")}
}
impl Drop for Duplex {fn drop(&mut self) {self.0.lock().unwrap().drops+=1;}}
type Seen=Arc<Mutex<Trace>>;
fn step_unexpected(_:Outstanding<'_,'_>)->Result<()> {panic!("an unimplemented handler must not receive this frame")}
fn take(t:&Seen)->Vec<u8> {std::mem::take(&mut t.lock().unwrap().out)}
fn give(t:&Seen,p:&[u8]) {t.lock().unwrap().input.extend(p.iter().copied());}
fn peer(run:&RunningServerGuid)->(Peer,Seen,Vec<u8>,Vec<u8>) {
    let [mut neg,..]=synthetic_exchange();neg[14]=4;let raw=frame(&neg);
    let t=Arc::new(Mutex::new(Trace::default()));give(&t,&raw);
    let p=Peer::open(run,DirectStream::Fixture(Box::new(Duplex(Arc::clone(&t)))),IpAddr::from([192,0,2,51])).unwrap();
    let rep=take(&t);(p,t,raw,rep)
}
fn fold(h:&mut [u8;64],wire:&[u8]) {
    assert_eq!(&wire[..4],&((wire.len()-4) as u32).to_be_bytes());
    let digest=Sha512::new().chain_update(*h).chain_update(&wire[4..]).finalize();h.copy_from_slice(&digest[..]);
}
fn one_turn(p:&mut Peer,t:&Seen,token:&[u8],mid:u64,sid:u64,want:Turn,h:&mut [u8;64])->Vec<u8> {
    let q=frame(&body(token,mid,sid));give(t,&q);fold(h,&q);
    assert_eq!(p.poll_with(Issuer::FixedChallenge,step_unexpected).unwrap(),want);
    let r=take(t);assert_eq!(&r[28..36],&mid.to_le_bytes());
    if want!=Turn::FinalWritten {fold(h,&r);}r
}
fn tag_for_this_fixture(key:&[u8],smb:&[u8])->[u8;16] {
    let mut clear=smb.to_vec();clear[48..64].fill(0);let mut mac=Cmac::<Aes128>::new_from_slice(key).unwrap();
    Mac::update(&mut mac,&clear);let mut tag=[0;16];tag.copy_from_slice(mac.finalize().as_bytes());tag
}

#[test]
fn accepted_socket_reads_and_submits_each_of_the_three_strict_exchange_forms_then_a_signed_echo() {
    for style in 0..3 {
        let run=provider();let (mut p,t,neg,neg_r)=peer(&run);let mut h=[0;64];fold(&mut h,&neg);fold(&mut h,&neg_r);
        assert_eq!(p.poll(step_unexpected).unwrap(),Turn::Idle);assert!(take(&t).is_empty());
        let (optimistic,last)=ChallengeService::fixture_messages(style==1);
        let (pick,continued,selected_last)=ChallengeService::fixture_selected_messages();
        let (last,mid)=if style==2 {
            let r=one_turn(&mut p,&t,&pick,1,0,Turn::SelectionWritten,&mut h);
            assert_eq!(&r[12..16],&0xc0000016u32.to_le_bytes());assert_eq!(&r[44..52],&1u64.to_le_bytes());
            one_turn(&mut p,&t,&continued,2,1,Turn::ChallengeWritten,&mut h);
            (selected_last,3)
        } else {
            one_turn(&mut p,&t,&optimistic,1,0,Turn::ChallengeWritten,&mut h);(last,2)
        };
        let reply=one_turn(&mut p,&t,&last,mid,1,Turn::FinalWritten,&mut h);
        assert_eq!(&reply[12..16],&[0;4]);assert_eq!(reply[20]&9,9);
        // Sole public exported key for this invented reference exchange. Do
        // not ask the server for a key, sign with a live session or hash its final response.
        let known=[0xf7,0xbc,0xf4,0x5a,0x16,0x7d,0x64,0xd4,0x09,0x65,0x48,0xda,0xf3,0xc0,0x59,0x5a];
        let mut kdf=Hmac::<Sha256>::new_from_slice(&known).unwrap();
        for part in [&[0,0,0,1][..],b"SMBSigningKey\0\0",&h[..],&[0,0,0,128]] {Mac::update(&mut kdf,part);}
        let block=kdf.finalize();let key=&block.as_bytes()[..16];
        assert_eq!(&reply[52..68],&tag_for_this_fixture(key,&reply[4..]));
        assert_eq!(p.poll(step_unexpected).unwrap(),Turn::Idle);
        let mut echo=vec![0;68];echo[..4].copy_from_slice(b"\xfeSMB");echo[4]=64;echo[6]=1;echo[12]=13;
        echo[14]=1;echo[16]=8;echo[24..32].copy_from_slice(&(mid+1).to_le_bytes());echo[40]=1;echo[64]=4;
        let tag=tag_for_this_fixture(key,&echo);echo[48..64].copy_from_slice(&tag);give(&t,&frame(&echo));
        assert_eq!(p.poll(step_unexpected).unwrap(),Turn::EchoWritten);let ans=take(&t);
        assert_eq!(&ans[..4],&68u32.to_be_bytes());assert_eq!(&ans[12..16],&[0;4]);
        assert_eq!(&ans[52..68],&tag_for_this_fixture(key,&ans[4..]));assert_eq!(p.connection.in_flight,0);
        // A failed *different* authentication on this same physical stream must
        // not revoke the already valid session or let the failed case use its key.
        let (new_one,mut bad)=ChallengeService::fixture_messages(false);give(&t,&frame(&body(&new_one,mid+2,0)));
        assert_eq!(p.poll_with(Issuer::FixedChallenge,step_unexpected).unwrap(),Turn::ChallengeWritten);
        assert_eq!(&take(&t)[44..52],&2u64.to_le_bytes());
        bad[72]^=1;give(&t,&frame(&body(&bad,mid+3,2)));assert_eq!(p.poll(step_unexpected).unwrap(),Turn::ErrorWritten);
        assert_eq!(&take(&t)[12..16],&0xc000006du32.to_le_bytes());
        echo[24..32].copy_from_slice(&(mid+4).to_le_bytes());let tag=tag_for_this_fixture(key,&echo);
        echo[48..64].copy_from_slice(&tag);give(&t,&frame(&echo));assert_eq!(p.poll(step_unexpected).unwrap(),Turn::EchoWritten);
        let again=take(&t);assert_eq!(&again[52..68],&tag_for_this_fixture(key,&again[4..]));
    }
}

#[test]
fn same_connection_can_retry_after_a_real_negative_status_without_obtaining_any_session() {
    let run=provider();let (mut a,ta,_,_)=peer(&run);let (mut b,tb,_,_)=peer(&run);
    let (one,mut last)=ChallengeService::fixture_messages(false);
    give(&ta,&frame(&body(&one,1,0)));
    assert_eq!(a.poll_with(Issuer::FixedChallenge,step_unexpected).unwrap(),Turn::ChallengeWritten);
    take(&ta);give(&tb,&frame(&body(&last,1,1)));
    assert_eq!(b.poll(step_unexpected).unwrap(),Turn::ErrorWritten);
    assert_eq!(&take(&tb)[12..16],&0xc0000203u32.to_le_bytes()); // b cannot consume a's challenge
    last[72]^=1;give(&ta,&frame(&body(&last,2,1)));
    assert_eq!(a.poll(step_unexpected).unwrap(),Turn::ErrorWritten);
    let bad=take(&ta);assert_eq!(&bad[..4],&72u32.to_be_bytes());assert_eq!(&bad[12..16],&0xc000006du32.to_le_bytes());
    assert_eq!(&bad[68..],&[9,0,0,0,0,0,0,0]);assert!(a.connection.sessions.is_empty());
    assert_eq!(a.connection.window.available(),4);
    give(&ta,&frame(&body(&one,3,0)));assert_eq!(a.poll(step_unexpected).unwrap(),Turn::ChallengeWritten);
    assert_eq!(&take(&ta)[44..52],&2u64.to_le_bytes()); // fresh real challenge; no state or ID resurrected
    let (mut none,tn,_,_)=peer(&running([9;16]));give(&tn,&frame(&body(&one,1,0)));
    assert_eq!(none.poll(step_unexpected).unwrap(),Turn::ErrorWritten);
    assert_eq!(&take(&tn)[12..16],&0x8009030eu32.to_le_bytes());
}

#[test]
fn quiet_boundary_partial_timeout_negotiated_limit_and_unsettled_special_setup_are_distinct() {
    let run=provider();
    for kind in 0..5 {
        let (mut p,t,_,_)=peer(&run);assert_eq!(p.poll(step_unexpected).unwrap(),Turn::Idle);
        match kind {
            0=>give(&t,&[0]),1=>give(&t,&[0,0,0,70,0xfe,0x53]),
            2=>{let mut q=65_793u32.to_be_bytes().to_vec();q.push(0xcc);give(&t,&q);},
            3=>t.lock().unwrap().eof=true,
            _=>{p.first_login_by=Some(Instant::now());},
        }
        assert!(p.poll(step_unexpected).is_err());assert!(take(&t).is_empty());
        assert_eq!(t.lock().unwrap().drops,1);if kind==2 {assert_eq!(t.lock().unwrap().input.len(),1);}
    }
    let (mut p,t,_,_)=peer(&run);
    let (one,_)=ChallengeService::fixture_messages(false);let mut q=body(&one,1,0);
    q[66]=1;give(&t,&frame(&q)); // binding must NOT become a new unsigned login
    let err=p.poll(|item|{assert!(matches!(item,Outstanding::SpecialSetup(_)));Ok(())});
    assert_eq!(err,Err(Fault::Closed));assert!(take(&t).is_empty());assert_eq!(t.lock().unwrap().drops,1);
}
