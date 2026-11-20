//! Synthetic accounting headers only. These are not signed responses,
//! storage operations, network traffic, or interoperability evidence.
use super::*;
use crate::smb::wire::{direct::Frame, preauth::tests::synthetic_exchange};

fn p16(p: &mut [u8], at: usize, n: u16) { p[at..at+2].copy_from_slice(&n.to_le_bytes()); }
fn p32(p: &mut [u8], at: usize, n: u32) { p[at..at+4].copy_from_slice(&n.to_le_bytes()); }
fn p64(p: &mut [u8], at: usize, n: u64) { p[at..at+8].copy_from_slice(&n.to_le_bytes()); }
fn parsed<T>(p: &[u8], f: impl FnOnce(&Compound<'_>) -> T) -> T {
    let mut stream = (p.len() as u32).to_be_bytes().to_vec(); stream.extend_from_slice(p);
    let (frame, tail) = Frame::split(&stream).unwrap().unwrap(); assert!(tail.is_empty());
    f(&Compound::parse(frame).unwrap())
}
fn opening(ask: u16) -> (Opening, Vec<u8>) {
    let [mut req, mut rsp, ..] = synthetic_exchange();
    p16(&mut req,14,ask);
    let opening = parsed(&req, |f| Opening::received(f).unwrap());
    p16(&mut rsp,14,opening.credit_response());
    (opening,rsp)
}
fn ready(ask: u16) -> Connection {
    let (x,p) = opening(ask);
    let tx = x.bind(p.clone()).unwrap(); assert_eq!(tx.bytes(),p.as_slice()); tx.submitted().unwrap()
}
fn request(cmd: u16, mid: u64, charge: u16, ask: u16) -> Vec<u8> {
    let mut p = vec![0;120]; p[..4].copy_from_slice(b"\xfeSMB");
    p16(&mut p,4,64); p16(&mut p,6,charge); p16(&mut p,12,cmd); p16(&mut p,14,ask);
    p64(&mut p,24,mid); p16(&mut p,64,4); p
}
fn chain(mut list: Vec<Vec<u8>>) -> Vec<u8> {
    let count = list.len();
    let mut out = Vec::new();
    for (i,v) in list.iter_mut().enumerate() {
        if i+1 != count {
            let n = v.len().next_multiple_of(8); v.resize(n,0); p32(v,20,n as u32);
        }
        out.extend_from_slice(v);
    }
    out
}
fn accept(c: &mut Connection, members: Vec<Vec<u8>>) -> Result<Debits> {
    parsed(&chain(members), |f| c.receive(f))
}
fn response(cmd: u16, mid: u64, grant: u16, status: u32) -> Vec<u8> {
    let mut p = request(cmd,mid,0,grant); p32(&mut p,8,status); p32(&mut p,16,1); p
}
fn settle(c: &mut Connection, b: &mut Debits) -> Vec<u16> {
    let batch: Vec<_> = b.entries.iter().enumerate().filter(|(_,m)| m.cmd != CANCEL)
        .map(|(i,m)| (i,m.cmd,m.mid,m.payload)).collect();
    let plan = c.plan(b).unwrap();
    let mut ns = Vec::new();
    let replies = batch.into_iter().map(|(i,cmd,mid,payload)| {
        let n = plan.credit_response(i).unwrap(); ns.push(n);
        response(cmd,mid,n,if payload == Payload::InvalidParameter { 0xc000000d } else { 0 })
    }).collect();
    let actual = chain(replies);
    let bound = plan.bind(&actual).unwrap();
    assert_eq!(bound.bytes(),actual.as_slice());
    bound.submitted().unwrap();
    ns
}

#[test]
fn fresh_445_admits_only_message_zero_and_exact_negotiated_grant() {
    for (ask,want) in [(0,1),(5,5),(u16::MAX,CAP)] {
        let (x,mut p) = opening(ask); assert_eq!(x.credit_response(),want);
        p16(&mut p,14,0); assert!(matches!(x.bind(p),Err(Fault::Opening)));
        let c = ready(ask); assert_eq!((c.window.low,c.window.next,c.window.available()),(1,1+u64::from(want),want));
        assert!(c.negotiation().is_ok());
    }
    for at in [88,92,96,100] {
        let (x,mut p) = opening(1); p32(&mut p,at,0);
        assert!(matches!(x.bind(p),Err(Fault::Opening)));
    }
    let (x,mut p) = opening(1); p32(&mut p,100,1024*1024);
    assert!(matches!(x.bind(p), Err(Fault::Opening))); // cannot receive a whole advertised write
    let (x,mut p) = opening(1);
    p32(&mut p,92,128*1024); p32(&mut p,96,256*1024); p32(&mut p,100,96*1024);
    let limits = x.bind(p).unwrap().submitted().unwrap().limits;
    assert_eq!((limits.transact,limits.read,limits.write),(128*1024,256*1024,96*1024));
    let [mut multi,rsp,..] = synthetic_exchange();
    p16(&mut multi,66,2); p16(&mut multi,100,0x0302); p16(&mut multi,102,0x0311);
    let x = parsed(&multi,|f| Opening::received(f).unwrap());
    let mut c = x.bind(rsp).unwrap().submitted().unwrap();
    assert_eq!(c.client.dialects_le.as_slice(), &[2,3,17,3]);
    assert_eq!((c.client.security_mode,c.client.capabilities,c.client.guid[0]),(3,4,0xc0));
    assert_eq!((c.limits.transact,c.limits.read,c.limits.write),(65_536,65_536,65_536));
    assert!(matches!(accept(&mut c,vec![request(0,1,1,1)]),Err(Fault::Opening)));
    let [mut req,..] = synthetic_exchange();
    p64(&mut req,24,1);
    assert!(parsed(&req, |f| matches!(Opening::received(f),Err(Fault::Opening))));
    let mut c = ready(3);
    assert!(matches!(accept(&mut c,vec![request(13,0,1,1)]),Err(Fault::Range)));
    assert!(matches!(c.negotiation(),Err(Fault::Closed)));
}

#[test]
fn out_of_order_and_multicredit_take_only_actual_available_sequence_numbers() {
    let mut c = ready(4);
    let mut third = accept(&mut c,vec![request(13,3,1,3)]).unwrap();
    let mut first = accept(&mut c,vec![request(8,1,2,0)]).unwrap(); // covers 1 and 2
    assert_eq!(c.window.available(),1);
    assert_eq!(settle(&mut c,&mut third),[3]); // original 4 plus new 5,6,7
    assert_eq!(settle(&mut c,&mut first),[0]);
    let mut late = accept(&mut c,vec![request(13,7,1,0)]).unwrap();
    assert_eq!(settle(&mut c,&mut late),[0]);
    assert!(matches!(accept(&mut c,vec![request(8,2,1,1)]),Err(Fault::Range)));

    let mut full = ready(CAP);
    let mut last = accept(&mut full,vec![request(13,64,1,64)]).unwrap();
    assert_eq!(settle(&mut full,&mut last),[0]); // lowest hole bounds the span
    let mut low = accept(&mut full,vec![request(13,1,1,64)]).unwrap();
    assert_eq!(settle(&mut full,&mut low),[1]); // only MID 65 newly available
}

#[test]
fn compound_never_spends_a_prospective_grant_nor_reuses_ids_or_inherits_them() {
    for (credits,mid1,n,mid2) in [(1,1,1,2),(3,1,2,2),(3,1,1,1)] {
        let mut c = ready(credits);
        let a = request(13,mid1,n,64); let mut b = request(13,mid2,1,64); p32(&mut b,16,4);
        assert!(matches!(accept(&mut c,vec![a,b]),Err(Fault::Range)));
        assert_eq!(c.in_flight,0); // no partially dispatchable batch
        assert!(matches!(accept(&mut c,vec![request(13,1,1,1)]),Err(Fault::Closed)));
    }
    let mut c = ready(3);
    let a = request(8,2,2,0); let mut b = request(13,1,1,0); p32(&mut b,16,4);
    let mut both = accept(&mut c,vec![a,b]).unwrap();
    assert_eq!(both.payload(0),Some(Payload::Covered));
    assert_eq!(both.payload(1),Some(Payload::NoVariableRule));
    assert_eq!(settle(&mut c,&mut both),[1,0]); // keep a last usable credit
}

#[test]
fn cancel_is_unchecked_for_sequence_only_and_never_answers_or_allocates() {
    let mut c = ready(1);
    let mut original = accept(&mut c,vec![request(13,1,1,0)]).unwrap();
    for flag in [0,2] {
        let mut m = request(CANCEL,1,u16::MAX,u16::MAX); p32(&mut m,16,flag);
        if flag == 2 { p64(&mut m,32,99); }
        let mut exempt = accept(&mut c,vec![m]).unwrap();
        assert_eq!(exempt.payload(0),Some(Payload::CancelExempt));
        assert!(matches!(c.plan(&mut exempt),Err(Fault::NoReply)));
    }
    assert_eq!((c.window.available(),c.in_flight),(0,1));
    assert_eq!(settle(&mut c,&mut original),[1]);
    let mut mixed = accept(&mut c,vec![request(CANCEL,1,0,64),request(13,2,1,2)]).unwrap();
    assert_eq!(mixed.payload(0),Some(Payload::CancelExempt));
    assert_eq!(settle(&mut c,&mut mixed),[2]);
}

#[test]
fn payload_uses_original_lengths_and_fails_member_not_transport() {
    for (cmd,at) in [(8,68),(9,68),(14,92),(15,68),(16,68),(16,76),(17,68)] {
        for (charge,n,want) in [(0,65_536,Payload::Covered),(0,65_537,Payload::InvalidParameter),
            (1,65_537,Payload::InvalidParameter),(2,65_537,Payload::Covered)] {
            let mut c = ready(4); let mut m = request(cmd,1,charge,1); p32(&mut m,at,n);
            let mut b = accept(&mut c,vec![m]).unwrap(); assert_eq!(b.payload(0),Some(want));
            if want == Payload::InvalidParameter {
                let plan = c.plan(&mut b).unwrap(); let g = plan.credit_response(0).unwrap();
                assert!(matches!(plan.bind(&response(cmd,1,g,0)),Err(Fault::WrongReply)));
            }
            settle(&mut c,&mut b); assert!(c.negotiation().is_ok());
        }
    }
    for (a,b) in [(92,104),(96,108)] {
        let mut c = ready(4); let mut m = request(11,1,2,1);
        p32(&mut m,a,u32::MAX); p32(&mut m,b,1); // a u32 sum would wrap to zero
        let mut d = accept(&mut c,vec![m]).unwrap();
        assert_eq!(d.payload(0),Some(Payload::InvalidParameter)); settle(&mut c,&mut d);
    }
    let mut c = ready(1); let mut short = request(14,1,1,0); short.truncate(70);
    let mut badly = accept(&mut c,vec![short]).unwrap();
    assert_eq!(badly.payload(0),Some(Payload::InvalidParameter)); settle(&mut c,&mut badly);
}

#[test]
fn only_matching_actual_synchronous_reply_can_publish_grants_exactly_once() {
    let mut c = ready(2); let mut other = ready(2);
    let mut b = accept(&mut c,vec![request(13,1,1,2),request(4,2,1,2)]).unwrap();
    assert!(matches!(other.plan(&mut b),Err(Fault::WrongBatch)));
    for variant in 0..7 {
        let plan = c.plan(&mut b).unwrap();
        let (g,h) = (plan.credit_response(0).unwrap(),plan.credit_response(1).unwrap());
        let mut a = response(13,1,g,0); let mut z = response(4,2,h,0);
        match variant {
            0 => p64(&mut a,24,2), 1 => p16(&mut z,12,13), 2 => p16(&mut z,14,h+1),
            3 => p32(&mut a,16,3), 4 => p32(&mut a,8,0x103), _ => {},
        }
        let mut bytes = chain(vec![a,z]);
        if variant == 5 { p32(&mut bytes,20,0); }
        if variant == 6 { p32(&mut bytes,20,71); }
        assert!(matches!(plan.bind(&bytes),Err(Fault::WrongReply)));
    }
    assert_eq!((c.window.available(),c.in_flight),(0,2));
    assert_eq!(settle(&mut c,&mut b),[2,2]);
    assert_eq!((c.window.available(),c.in_flight),(4,0));
    assert!(matches!(c.plan(&mut b),Err(Fault::NoReply)));
    let mut second = accept(&mut c,vec![request(13,3,1,1)]).unwrap();
    let plan = c.plan(&mut second).unwrap(); let g = plan.credit_response(0).unwrap();
    let bytes = response(13,3,g,0); drop(plan.bind(&bytes).unwrap()); // ambiguous send
    assert!(matches!(c.negotiation(),Err(Fault::Closed)));
    assert!(other.negotiation().is_ok());
}

#[test]
fn forgotten_ordinary_reply_frame_limits_and_number_exhaustion_end_only_that_transport() {
    let mut c = ready(1);
    drop(accept(&mut c,vec![request(13,1,1,0)]).unwrap());
    assert!(matches!(c.negotiation(),Err(Fault::Closed)));
    let mut c = ready(1);
    let mut oversized = request(9,1,1,1); oversized.resize(65_536+257,0);
    assert!(matches!(accept(&mut c,vec![oversized]),Err(Fault::FrameSize)));
    let (x,mut p) = opening(1); p32(&mut p,92,128*1024);
    let mut c = x.bind(p).unwrap().submitted().unwrap();
    let mut small = request(13,1,1,1); small.resize(68*1024+1,0);
    assert!(matches!(accept(&mut c,vec![small]),Err(Fault::FrameSize)));
    let mut c = ready(1);
    c.window = Window { low:u64::MAX-1,next:u64::MAX,bits:1 }; // last possible usable ID
    let mut last = accept(&mut c,vec![request(13,u64::MAX-1,1,1)]).unwrap();
    assert!(matches!(c.plan(&mut last),Err(Fault::Exhausted)));
    assert!(matches!(c.negotiation(),Err(Fault::Closed)));
    let mut n = Window { low:u64::MAX-1,next:u64::MAX,bits:1 };
    assert!(!n.take(u64::MAX-1,2)); assert!(!n.take(u64::MAX,1));
}
