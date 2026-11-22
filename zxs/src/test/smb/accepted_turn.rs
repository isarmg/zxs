//! No listener or accounts. A single synthetic full-duplex stream carries
//! negotiation through the final strict packet and signed ordinary replies.
//! The test-only challenge is public fixed data; the salt/time in the real
//! NEGOTIATE serializer are not predicted. Compare against the bytes written.
use super::*;
use crate::smb::{connection::{sessions::{RunningServerGuid,challenge::tests::provider,tests::running},
    setup::tests::{body,frame},transport::DirectStream},session::{ChallengeService,HmacForSessionFixture as Hmac},
    wire::preauth::tests::synthetic_exchange};
use crate::smb::session::SigningMacForSessionFixture;
use crate::smb::fixed_output::{KeyInit,Mac};
use crate::smb::handshake_hash::{Sha256,Sha512};
use crate::smb::fixed_output::Digest;
use std::{collections::VecDeque,io::{self,ErrorKind,Read,Write},sync::{Arc,Mutex}};

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
    let p=Peer::open_fixture(run,DirectStream::Fixture(Box::new(Duplex(Arc::clone(&t))))).unwrap();
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
    let mut clear=smb.to_vec();clear[48..64].fill(0);let mut mac=SigningMacForSessionFixture::new_from_slice(key).unwrap();
    Mac::update(&mut mac,&clear);let mut tag=[0;16];tag.copy_from_slice(mac.finalize().as_bytes());tag
}

fn signing_key_for_this_fixture(h:&[u8;64])->[u8;16] {
    // Exported key of the published synthetic exchange, not read from the owner.
    let known=[0xf7,0xbc,0xf4,0x5a,0x16,0x7d,0x64,0xd4,0x09,0x65,0x48,0xda,0xf3,0xc0,0x59,0x5a];
    let mut kdf=Hmac::<Sha256>::new_from_slice(&known).unwrap();
    for part in [&[0,0,0,1][..],b"SMBSigningKey\0\0",&h[..],&[0,0,0,128]] {Mac::update(&mut kdf,part);}
    kdf.finalize().as_bytes()[..16].try_into().unwrap()
}
fn tree_request(key:&[u8],mid:u64,path:&str,offset:u16)->Vec<u8> {
    let mut request=vec![0;72];request[..4].copy_from_slice(b"\xfeSMB");
    request[4]=64;request[6]=1;request[12]=3;request[14]=2;request[16]=0x28;
    request[24..32].copy_from_slice(&mid.to_le_bytes());request[32..36].copy_from_slice(&0x44332211u32.to_le_bytes());
    request[40]=1;request[64]=9;request[68..70].copy_from_slice(&offset.to_le_bytes());
    for unit in path.encode_utf16() {request.extend_from_slice(&unit.to_le_bytes());}
    let n=(request.len()-72) as u16;request[70..72].copy_from_slice(&n.to_le_bytes());
    let tag=tag_for_this_fixture(key,&request);request[48..64].copy_from_slice(&tag);
    frame(&request)
}

fn authenticated_fixture(run:&RunningServerGuid)->(Peer,Seen,[u8;16]) {
    let (mut p,t,neg,rep)=peer(run);
    let mut h=[0;64];fold(&mut h,&neg);fold(&mut h,&rep);
    let (one,three)=ChallengeService::fixture_messages(false);
    one_turn(&mut p,&t,&one,1,0,Turn::ChallengeWritten,&mut h);
    one_turn(&mut p,&t,&three,2,1,Turn::FinalWritten,&mut h);
    (p,t,signing_key_for_this_fixture(&h))
}
fn fixed_request(command:u16,mid:u64,flags:u8,routing:u64)->Vec<u8> {
    let mut p=vec![0;68];p[..4].copy_from_slice(b"\xfeSMB");p[4]=64;p[6]=1;p[14]=1;
    p[12..14].copy_from_slice(&command.to_le_bytes());p[16]=flags;
    p[24..32].copy_from_slice(&mid.to_le_bytes());p[32..40].copy_from_slice(&routing.to_le_bytes());
    p[40]=1;p[64]=4;p
}
fn cancel_request(mid:u64,flags:u8,routing:u64)->Vec<u8> {
    let mut p=fixed_request(CANCEL,mid,flags,routing);
    p[6..8].fill(255);p[14..16].fill(255); // neither spend nor grant anything
    p[66..68].copy_from_slice(&[0x37,0xe1]); // ignored only as a field, never removed from CMAC
    p
}
fn seal_fixture_request(p:&mut [u8],key:&[u8]) {
    let tag=tag_for_this_fixture(key,p);p[48..64].copy_from_slice(&tag);
}
fn echo_turn(p:&mut Peer,t:&Seen,key:&[u8],mid:u64) {
    let mut q=fixed_request(13,mid,8,0);seal_fixture_request(&mut q,key);
    give(t,&frame(&q));assert_eq!(p.poll(step_unexpected),Ok(Turn::EchoWritten));
    let ans=take(t);assert_eq!(&ans[28..36],&mid.to_le_bytes());
    assert_eq!(&ans[52..68],&tag_for_this_fixture(key,&ans[4..]));
}

#[test]
fn signed_standalone_cancel_without_target_has_no_output_or_credit_and_keeps_original_session() {
    let run=provider();let (mut p,t,key)=authenticated_fixture(&run);
    echo_turn(&mut p,&t,&key,3);
    let before=p.connection.window;
    for (mid,flags,routing) in [(3,8,0),(3,8,0),(4,8,0), (u64::MAX-2,8,0), (0,10,0x0123_4567_89ab_cdef)] {
        let mut b=cancel_request(mid,flags,routing);seal_fixture_request(&mut b,&key);
        give(&t,&frame(&b));assert_eq!(p.poll(step_unexpected),Ok(Turn::CancelNoTarget));
        assert!(take(&t).is_empty());assert_eq!(t.lock().unwrap().drops,0);
        let now=p.connection.window;
        assert_eq!((now.low,now.next,now.bits),(before.low,before.next,before.bits));
        assert_eq!(p.connection.in_flight,0);assert_eq!(p.connection.sessions.len(),1);
    }
    echo_turn(&mut p,&t,&key,4); // CANCEL did not spend this original available MID
}

#[test]
fn malformed_unverified_or_compounded_cancel_never_reaches_a_handler() {
    for kind in 0..7 {
        let run=provider();let (mut p,t,key)=authenticated_fixture(&run);
        let mut b=cancel_request(3,8,0);
        let want=match kind {
            0=>{b[16]=0;Fault::SessionState}, // even a tag under the real fixture key does not set SIGNED
            1=>Fault::SessionState,
            2=>{b[64]=3;Fault::WrongReply},
            3=>{b.pop();Fault::WrongReply},
            4=>{b.push(0);Fault::WrongReply},
            5=>Fault::WrongReply,
            _=>{run.fixture_revoke_provider();Fault::NoVerifier},
        };
        seal_fixture_request(&mut b,&key);
        if kind==1 {b[66]^=1;} // bytes of the reserved word still authenticated
        if kind==5 {
            let mut first=fixed_request(13,3,8,0);first.resize(72,0);first[20]=72;
            seal_fixture_request(&mut first,&key);first.extend_from_slice(&b);b=first;
        }
        give(&t,&frame(&b));assert_eq!(p.poll(step_unexpected),Err(want));
        assert!(take(&t).is_empty());assert_eq!(t.lock().unwrap().drops,1);
    }
}

#[test]
fn only_the_receiving_peer_can_take_the_exact_ordinary_move_capture(){
    let run=provider();
    let (mut a,trace,_,_)=peer(&run);
    let (mut b,other,_,_)=peer(&run);
    let same_bytes=frame(&fixed_request(13,1,0,0));
    give(&trace,&same_bytes);
    let captured=OriginalMoveInput::new(CapturedRecord::from_stream(&mut a.connection,MAX_FRAME_BYTES).unwrap().unwrap());
    {
        let original=captured.frame_on(&a.connection).unwrap();
        let detached=parsed(&captured.capture.bytes).unwrap();
        let mark=MoveCapture{frame:&original,owned:&captured};
        assert!(mark.matches(&original));
        assert!(!mark.matches(&detached)); // equal bytes are not the borrowed original
        give(&trace,&same_bytes);
        let second=OriginalMoveInput::new(CapturedRecord::from_stream(&mut a.connection,MAX_FRAME_BYTES).unwrap().unwrap());
        assert!(captured.issued(&mark.seal()));
        assert!(!second.issued(&mark.seal())); // a distinct physical record, even from this peer
    }
    assert_eq!(b.park_original_move(captured.capture),Err(Fault::WrongBatch));
    assert!(b.original_move.is_none()&&a.original_move.is_none());
    assert_eq!((a.connection.alive(),b.connection.alive()),(Err(Fault::Closed),Ok(())));
    assert_eq!((a.connection.in_flight,b.connection.in_flight),(0,0));
    assert!(take(&trace).is_empty()&&take(&other).is_empty());
}

#[test]
fn pending_framer_waits_for_one_complete_original_without_spending_or_taking_its_suffix(){
    use crate::smb::connection::transport::PendingRead;
    let run=provider();let (mut p,t,key)=authenticated_fixture(&run);
    let mut q=cancel_request(3,8,0);
    seal_fixture_request(&mut q,&key);
    let first=frame(&q);
    let next=frame(&fixed_request(13,4,8,0));
    let mut input=PendingRead::default();
    let before=p.connection.window;
    give(&t,&first[..2]);
    assert!(CapturedRecord::from_pending(&mut p.connection,&mut input).unwrap().is_none());
    assert!(CapturedRecord::from_pending(&mut p.connection,&mut input).unwrap().is_none()); // quiet partial
    give(&t,&first[2..]);give(&t,&next);
    let captured=(0..32).find_map(|_|CapturedRecord::from_pending(&mut p.connection,&mut input).unwrap()).unwrap();
    assert_eq!(captured.bytes,first);
    assert!(captured.frame_on(&p.connection).is_ok());
    assert_eq!(t.lock().unwrap().input.iter().copied().collect::<Vec<_>>(),next);
    let after=p.connection.window;
    assert_eq!((before.low,before.next,before.bits),(after.low,after.next,after.bits));
    assert_eq!(p.connection.in_flight,0);
    assert!(take(&t).is_empty());
}

#[test]
fn pending_framer_does_not_convert_idle_or_partial_eof_into_a_record(){
    use crate::smb::connection::transport::PendingRead;
    let run=provider();
    for partial in [&[][..],&[0,0][..],&[0,0,0,68,0xfe][..]] {
        let (mut p,t,_,_)=peer(&run);
        let mut input=PendingRead::default();
        give(&t,partial);
        assert!(CapturedRecord::from_pending(&mut p.connection,&mut input).unwrap().is_none());
        assert!(CapturedRecord::from_pending(&mut p.connection,&mut input).unwrap().is_none());
        t.lock().unwrap().eof=true;
        assert!(matches!(CapturedRecord::from_pending(&mut p.connection,&mut input),Err(Fault::Closed)));
        assert_eq!(p.connection.in_flight,0);
        assert!(take(&t).is_empty());
    }
}

#[test]
fn signed_move_shape_without_an_original_file_cannot_park_or_release_its_debt(){
    let run=provider();let (mut p,t,key)=authenticated_fixture(&run);
    let mut q=fixed_request(17,3,8,1u64<<32);q.resize(122,0);
    q[64]=33;q[66]=1;q[67]=10;q[68]=26;q[72]=96;q[80]=1;q[88]=2;q[112]=6;
    q[116..].copy_from_slice(&[b'G',0,b'\\',0,b'x',0]);
    seal_fixture_request(&mut q,&key);give(&t,&frame(&q));
    let previous=p.connection.window.next;
    let captured=CapturedRecord::from_stream(&mut p.connection,MAX_FRAME_BYTES).unwrap().unwrap();
    assert_eq!(p.park_original_move(captured),Err(Fault::WrongReply));
    assert!(p.original_move.is_none()&&p.connection.sessions.is_empty());
    assert_eq!((p.connection.in_flight,p.connection.window.next,p.connection.alive()),(1,previous,Err(Fault::Closed)));
    assert!(take(&t).is_empty());assert_eq!(t.lock().unwrap().drops,1);
}

#[test]
fn ordinary_move_router_refuses_missing_tree_on_the_original_paid_wire_and_continues(){
    let run=provider();let (mut p,t,key)=authenticated_fixture(&run);
    let mut q=fixed_request(17,3,8,1u64<<32);q.resize(122,0);
    q[64]=33;q[66]=1;q[67]=10;q[68]=26;q[72]=96;q[80]=1;q[88]=2;q[112]=6;
    q[116..].copy_from_slice(&[b'G',0,92,0,b'x',0]);
    seal_fixture_request(&mut q,&key);give(&t,&frame(&q));
    let previous=p.connection.window.next;
    assert_eq!(p.poll_selected(Issuer::Protected,true,step_unexpected),Ok(Turn::ErrorWritten));
    let ans=take(&t);
    assert_eq!(ans.len(),76); // one fixed ERROR response, no STATUS_PENDING
    assert_eq!(&ans[12..16],&0xc000_00c9u32.to_le_bytes()); // original Tree was not in this Session
    assert_eq!(&ans[28..36],&3u64.to_le_bytes());
    assert_eq!(&ans[52..68],&tag_for_this_fixture(&key,&ans[4..]));
    let granted=u16::from_le_bytes(ans[18..20].try_into().unwrap());
    assert_eq!(p.connection.window.next,previous+u64::from(granted));
    assert!(p.original_move.is_none());
    assert_eq!(p.connection.in_flight,0);
    let mut next=fixed_request(13,4,8,0);seal_fixture_request(&mut next,&key);
    give(&t,&frame(&next));
    assert_eq!(p.poll_selected(Issuer::Protected,true,step_unexpected),Ok(Turn::EchoWritten));
    assert_eq!(&take(&t)[28..36],&4u64.to_le_bytes());
}

#[test]
fn cancel_has_no_power_over_another_connection_or_an_unsettled_ordinary_receipt() {
    let run=provider();let (mut original,t,key)=authenticated_fixture(&run);
    let (mut foreign,wrong,_,_)=peer(&run);
    let mut cancel=cancel_request(3,8,0);seal_fixture_request(&mut cancel,&key);
    give(&wrong,&frame(&cancel));assert_eq!(foreign.poll(step_unexpected),Err(Fault::SessionState));
    assert!(take(&wrong).is_empty());assert_eq!(wrong.lock().unwrap().drops,1);
    echo_turn(&mut original,&t,&key,3);

    // Take an actual paid receipt without settling it. This is not an async
    // worker; it proves the synchronous no-target branch won't swallow it.
    let mut q=fixed_request(13,4,8,0);seal_fixture_request(&mut q,&key);
    let record=frame(&q);let parsed=parsed(&record).unwrap();
    let outstanding=original.connection.receive(&parsed).unwrap();
    assert!(outstanding.debit.owed);assert_eq!(original.connection.in_flight,1);
    cancel[24..32].copy_from_slice(&4u64.to_le_bytes());seal_fixture_request(&mut cancel,&key);
    give(&t,&frame(&cancel));assert_eq!(original.poll(step_unexpected),Err(Fault::WrongReply));
    assert!(outstanding.debit.owed);assert!(take(&t).is_empty());assert_eq!(t.lock().unwrap().drops,1);
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
        let key=&signing_key_for_this_fixture(&h);
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

#[test]
fn paid_tree_refusals_are_signed_and_a_valid_name_is_neither_consumed_nor_granted() {
    use crate::smb::connection::sessions::ordinary::Step;
    let run=provider();let (mut p,t,neg,rep)=peer(&run);
    let mut h=[0;64];fold(&mut h,&neg);fold(&mut h,&rep);
    let (one,three)=ChallengeService::fixture_messages(false);
    one_turn(&mut p,&t,&one,1,0,Turn::ChallengeWritten,&mut h);
    one_turn(&mut p,&t,&three,2,1,Turn::FinalWritten,&mut h);
    let key=&signing_key_for_this_fixture(&h);
    for (mid,path,offset,status) in [(3,r"\\fixture\IPC$",72,0xc000_00ccu32),
                                     (4,r"\\fixture\ZXS",71,0xc000_000d)] {
        let before=p.connection.window.available();
        give(&t,&tree_request(key,mid,path,offset));
        assert_eq!(p.poll(step_unexpected).unwrap(),Turn::TreeRefused);
        let actual=take(&t);let raw=&actual[4..];
        assert_eq!(&actual[..4],&72u32.to_be_bytes());
        assert_eq!(&raw[..8],b"\xfeSMB\x40\x00\x01\x00");
        assert_eq!(&raw[8..12],&status.to_le_bytes());
        assert_eq!(&raw[12..16],&[3,0,2,0]);
        assert_eq!(&raw[16..24],&[0x29,0,0,0,0,0,0,0]); // original priority retained; no compound
        assert_eq!(&raw[24..32],&mid.to_le_bytes());
        assert_eq!(&raw[32..36],&0x44332211u32.to_le_bytes());
        assert_eq!(&raw[36..40],&[0;4]); // no TreeId for negative response
        assert_eq!(&raw[40..48],&1u64.to_le_bytes());
        assert_eq!(&raw[48..64],&tag_for_this_fixture(key,raw));
        assert_eq!(&raw[64..],&[9,0,0,0,0,0,0,0]);
        assert_eq!(p.connection.in_flight,0);assert_eq!(p.connection.window.available(),before+1);
    }
    give(&t,&tree_request(key,5,r"\\fixture\ZXS",72));
    let settled=p.poll(|x|{
        let Outstanding::Ordinary(Admission::Ready(mut batch))=x else {panic!("a real request is still owed");};
        let Some(Step::Verified(v))=batch.next()? else {panic!("valid request was consumed");};
        assert_eq!(v.message().header.message,5);
        v.session().recheck()?;Ok(()) // no Core root in this source fixture; no claimed success
    });
    assert_eq!(settled,Err(Fault::Closed));assert!(take(&t).is_empty());
}
