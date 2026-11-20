//! In-memory duplex scripts only: absolute wire bounds, the exact SMB bytes
//! seen by preauth, and no negotiated authority on a failed physical write.
use super::*;
use crate::smb::{connection::{sessions::tests::running,setup::tests::frame},
    wire::preauth::tests::synthetic_exchange};
use sha2::{Digest,Sha512};
use std::{io::{self,ErrorKind,Read,Write},sync::Mutex,time::Instant};

#[derive(Default)]
struct Trace {input:Vec<u8>,pos:usize,out:Vec<u8>,reads:usize,writes:usize,drops:usize}
struct Script {trace:Arc<Mutex<Trace>>,stop:Option<(usize,bool)>}
impl Read for Script {
    fn read(&mut self,dst:&mut [u8])->io::Result<usize> {
        let mut t=self.trace.lock().unwrap();t.reads+=1;
        if t.reads==1 {return Err(ErrorKind::Interrupted.into());}
        let n=dst.len().min(7).min(t.input.len()-t.pos);
        dst[..n].copy_from_slice(&t.input[t.pos..t.pos+n]);t.pos+=n;Ok(n)
    }
}
impl Write for Script {
    fn write(&mut self,src:&[u8])->io::Result<usize> {
        let mut t=self.trace.lock().unwrap();t.writes+=1;
        if t.writes==1 {return Err(ErrorKind::Interrupted.into());}
        let room=if let Some((limit,zero))=self.stop {
            if t.out.len()>=limit {return if zero {Ok(0)} else {Err(ErrorKind::BrokenPipe.into())};}
            limit-t.out.len()
        } else {usize::MAX};
        let n=src.len().min(11).min(room);t.out.extend_from_slice(&src[..n]);Ok(n)
    }
    fn flush(&mut self)->io::Result<()> {panic!("plain TCP submission is not file persistence")}
}
impl Drop for Script {fn drop(&mut self) {self.trace.lock().unwrap().drops+=1;}}
fn script(input:Vec<u8>,stop:Option<(usize,bool)>)->(DirectStream,Arc<Mutex<Trace>>) {
    let trace=Arc::new(Mutex::new(Trace{input,..Trace::default()}));
    (DirectStream::Fixture(Box::new(Script{trace:Arc::clone(&trace),stop})),trace)
}

#[test]
fn one_received_stream_one_complete_reply_one_actual_transcript_and_no_session() {
    let run=running([0x93;16]);let [mut q,_,setup,..]=synthetic_exchange();q[14]=4;
    let first=frame(&q);let later=frame(&setup);let mut joined=first.clone();joined.extend_from_slice(&later);
    let (stream,trace)=script(joined,None);let mut p=Peer::open_fixture(&run,stream).unwrap();
    assert!(p.connection.origin.endpoints.is_none());assert!(p.connection.sessions.is_empty());assert_eq!(p.connection.window.available(),4);
    let out={let t=trace.lock().unwrap();
        assert_eq!(t.pos,first.len());assert_eq!(t.drops,0);assert!(t.reads>3 && t.writes>3);t.out.clone()};
    assert_eq!(&out[..4],&174u32.to_be_bytes());assert_eq!(out.len(),178);
    assert_eq!(&out[4..8],b"\xfeSMB");assert_eq!(&out[4+72..4+88],&[0x93;16]);
    let h1=Sha512::new().chain_update([0u8;64]).chain_update(&q).finalize();
    let h2=Sha512::new().chain_update(h1).chain_update(&out[4..]).finalize();
    let expected=Sha512::new().chain_update(h2).chain_update(&setup).finalize();
    let next=p.connection.stream.as_mut().unwrap().read_record(65_536+256,deadline().unwrap()).unwrap();
    assert_eq!(next,later);let fr=parsed(&next).unwrap();
    let observed=p.connection.origin.hash.new_attempt().received(&fr).unwrap().into_kdf_context();
    assert_eq!(observed.as_bytes().as_slice(),&expected[..]);
    drop(p);assert_eq!(trace.lock().unwrap().drops,1);
}

#[test]
fn reject_prefix_before_allocating_or_reading_body_and_do_not_reuse_an_ambiguous_stream() {
    let run=running([8;16]);let [q,..]=synthetic_exchange();
    for (wire,consumed) in [
        (vec![1,0,0,1,0x42],4),(vec![0,0,0,0,0x42],4),({
            let mut v=((MAX_OPENING_BYTES+1) as u32).to_be_bytes().to_vec();v.push(0x42);v
        },4),(vec![0,0,0],3),(vec![0,0,0,12,1,2],6),
    ] {
        let (stream,trace)=script(wire,None);assert!(Peer::open_fixture(&run,stream).is_err());
        let t=trace.lock().unwrap();assert_eq!(t.pos,consumed);assert!(t.out.is_empty());assert_eq!(t.drops,1);
    }
    let (mut stream,t)=script(frame(&q),None);
    assert_eq!(stream.read_record(MAX_OPENING_BYTES,Instant::now()),Err(Fault::Transport));
    assert_eq!(t.lock().unwrap().reads,0);drop(stream);
    for (limit,zero) in [(2,false),(4,true),(29,false),(177,true)] {
        let (stream,t)=script(frame(&q),Some((limit,zero)));
        assert!(matches!(Peer::open_fixture(&run,stream),Err(Fault::Transport)));
        let seen=t.lock().unwrap();assert_eq!(seen.out.len(),limit);assert_eq!(seen.drops,1);
    }
}

#[test]
fn capacity_is_shared_by_incomplete_and_negotiated_connections_before_any_read() {
    let run=running([0x62;16]);let [q,..]=synthetic_exchange();
    let (stream,t)=script(frame(&q),None);let peer=Peer::open_fixture(&run,stream).unwrap();
    let mut held:Vec<_>=(0..127).map(|_|AcceptSeat::reserve(&run.share()).unwrap()).collect();
    let (blocked,observed)=script(frame(&q),None);
    assert!(matches!(Peer::open_fixture(&run,blocked),Err(Fault::SessionLimit)));
    assert_eq!(observed.lock().unwrap().reads,0);
    drop(held.pop());let (next,_)=script(frame(&q),None);let next=Peer::open_fixture(&run,next).unwrap();
    assert!(matches!(AcceptSeat::reserve(&run.share()),Err(Fault::SessionLimit)));
    drop(peer);assert_eq!(t.lock().unwrap().drops,1);
    let _seat=AcceptSeat::reserve(&run.share()).unwrap();drop(next);
}

#[test]
fn incompatible_signing_list_is_answered_in_full_without_exposing_a_negotiated_peer() {
    let run=running([0x44;16]);let [mut q,..]=synthetic_exchange();
    q[96..98].copy_from_slice(&2u16.to_le_bytes());q.resize(q.len().next_multiple_of(8),0xb3);
    q.extend_from_slice(&[8,0,4,0,0,0,0,0,1,0,2,0]); // only GMAC; the server returns CMAC as required
    let (stream,t)=script(frame(&q),None);
    assert!(matches!(Peer::open_fixture(&run,stream),Err(Fault::NoCommonSigning)));
    let seen=t.lock().unwrap();assert_eq!(seen.out.len(),192);assert_eq!(&seen.out[..4],&188u32.to_be_bytes());
    assert_eq!(&seen.out[180..],&[8,0,4,0,0,0,0,0,1,0,1,0]);assert_eq!(seen.drops,1);
}

#[test]
fn shape_checks_both_ends_and_leaves_exact_interface_scope_for_policy() {
    use std::net::SocketAddrV6;
    let local="192.0.2.50:445".parse::<SocketAddr>().unwrap();
    let peer="192.0.2.42:51013".parse::<SocketAddr>().unwrap();
    let local6="[2001:db8:a::2]:445".parse::<SocketAddr>().unwrap();
    let peer6="[2001:db8:a::3]:51013".parse::<SocketAddr>().unwrap();
    assert!(direct_pair(local,peer));
    assert!(direct_pair(local6,peer6));
    let scoped_local=SocketAddr::V6(SocketAddrV6::new("fe80::1".parse().unwrap(),445,0,9));
    let scoped_peer=SocketAddr::V6(SocketAddrV6::new("fe80::2".parse().unwrap(),51013,0,9));
    assert!(direct_pair(scoped_local,scoped_peer)); // shape only, not a policy match
    assert!(!direct_pair(local,peer6));
    assert!(!direct_pair(local6,peer));
    for (l,r) in [
        ("0.0.0.0:445","192.0.2.42:51013"), ("[::]:445","[2001:db8:a::3]:51013"),
        ("192.0.2.50:139","192.0.2.42:51013"), ("224.0.0.1:445","192.0.2.42:51013"),
        ("[ff02::1]:445","[2001:db8:a::3]:51013"), ("255.255.255.255:445","192.0.2.42:51013"),
        ("[::ffff:0.0.0.0]:445","[::ffff:192.0.2.42]:51013"), ("[::ffff:224.0.0.1]:445","[::ffff:192.0.2.42]:51013"),
        ("192.0.2.50:445","192.0.2.42:0"), ("192.0.2.50:445","0.0.0.0:51013"),
        ("192.0.2.50:445","224.0.0.1:51013"), ("192.0.2.50:445","255.255.255.255:51013"),
        ("[2001:db8:a::2]:445","[ff02::1]:51013"), ("[2001:db8:a::2]:445","[::ffff:0.0.0.0]:51013"),
    ] {
        assert!(!direct_pair(l.parse().unwrap(),r.parse().unwrap()));
    }
}
