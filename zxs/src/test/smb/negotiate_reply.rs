//! Public deterministic fixtures, never credentials, a live server identity or
//! a network result. The reference octets were packed separately per MS-SMB2;
//! standard Python hashlib.sha512(previous + message) supplies the expected
//! chain after request, exact encoded response and first synthetic SETUP.
use super::*;
use crate::smb::wire::{direct::Frame, message::Compound, preauth::tests::synthetic_exchange};
use std::cell::Cell;

const DEFAULT_WIRE: &str = concat!(
    "fe534d4240000100000000000000050021000000000000000000000000000000",
    "8877665500000000000000000000000000000000000000000000000000000000",
    "410003001103010033221100554477468899aabbccddeeff0400000000000100",
    "0000010000000100875651d5deb19d0100000000000000008000000080000000",
    "0100260000000000010020000100808182838485868788898a8b8c8d8e8f9091",
    "92939495969798999a9b9c9d9e9f",
);
const EXPLICIT_WIRE: &str = concat!(
    "fe534d4240000100000000000000050021000000000000000000000000000000",
    "8877665500000000000000000000000000000000000000000000000000000000",
    "410003001103020033221100554477468899aabbccddeeff0400000000000100",
    "0000010000000100875651d5deb19d0100000000000000008000000080000000",
    "0100260000000000010020000100808182838485868788898a8b8c8d8e8f9091",
    "92939495969798999a9b9c9d9e9f0000080004000000000001000100",
);
const DEFAULT_HASH: &str = concat!(
    "a9343bf3cdeaaa3668d562fbe22d6d0c4f7177f285bbc642bc18a09b162bb225",
    "7c0f300c432bc24a9c44eadffe9c255289cbe1346107b39fbb779d67c3eb5177",
);
const EXPLICIT_HASH: &str = concat!(
    "de3056154706f40a486f2de1a2d7df200d421ef0c7a93acca9117bbe32a9e84a",
    "7fdcebd8270cbd5f03e4e204b623a1353676516763df62ff2156fb7c432ebfe6",
);

fn decode(s: &str) -> Vec<u8> {
    s.as_bytes().chunks_exact(2).map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(),16).unwrap()).collect()
}
fn framed<T>(p: &[u8], read: impl FnOnce(&Compound<'_>)->T) -> T {
    let mut tcp = (p.len() as u32).to_be_bytes().to_vec(); tcp.extend_from_slice(p);
    let (f,tail) = Frame::split(&tcp).unwrap().unwrap(); assert!(tail.is_empty());
    read(&Compound::parse(f).unwrap())
}
fn original() -> Vec<u8> {
    let [mut p, ..] = synthetic_exchange();
    p[14..16].copy_from_slice(&5u16.to_le_bytes());
    p[16..20].copy_from_slice(&0x20u32.to_le_bytes());
    p[32..36].copy_from_slice(&0x55667788u32.to_le_bytes());
    p[68..70].copy_from_slice(&1u16.to_le_bytes()); // client merely enables signing
    p[72..76].copy_from_slice(&0xfeu32.to_le_bytes()); // offers are not our capability bits
    p
}
fn context(p: &mut Vec<u8>, kind: u16, data: &[u8]) {
    let count = u16::from_le_bytes(p[96..98].try_into().unwrap()) + 1;
    p[96..98].copy_from_slice(&count.to_le_bytes());
    p.resize(p.len().next_multiple_of(8),0x71); // incoming padding is not reconstructed
    p.extend_from_slice(&kind.to_le_bytes()); p.extend_from_slice(&(data.len() as u16).to_le_bytes());
    p.extend_from_slice(&[0;4]); p.extend_from_slice(data);
}
fn opening(p: &[u8]) -> Result<Opening> { framed(p,Opening::received) }
fn server() -> RunningServerGuid {
    // GUID 00112233-4455-4677-8899-aabbccddeeff in MS-DTYP packet ordering.
    RunningServerGuid { wire: [0x33,0x22,0x11,0x00,0x55,0x44,0x77,0x46,0x88,0x99,0xaa,0xbb,0xcc,0xdd,0xee,0xff] }
}
fn local_time() -> SystemTime { UNIX_EPOCH + Duration::from_nanos(123_456_789) }
fn fixed(req: &[u8], guid: &RunningServerGuid, salt_start: u8) -> Result<OpeningSend> {
    opening(req)?.respond_from(guid, |salt| { *salt=std::array::from_fn(|i|salt_start+i as u8); Ok(()) }, local_time)
}

#[test]
fn default_and_explicit_cmac_encode_true_reply_publish_real_credits_and_hash_the_actual_bytes() {
    let [_,_,setup,_,_] = synthetic_exchange();
    for (explicit,expected,hash) in [(false,DEFAULT_WIRE,DEFAULT_HASH),(true,EXPLICIT_WIRE,EXPLICIT_HASH)] {
        let mut req = original();
        if explicit { context(&mut req,8,&[2,0,2,0,1,0]); } // GMAC is preferred, CMAC is common
        let tx = fixed(&req,&server(),0x80).unwrap();
        assert_eq!(tx.bytes(),decode(expected).as_slice());
        let mut conn = tx.submitted().unwrap();
        assert_eq!((conn.server.security_mode,conn.server.capabilities),(3,4));
        assert_eq!(conn.server.guid,server().wire);
        assert_eq!(conn.window.available(),5);
        // Debit the actual original MID=1 before the independent transcript fork;
        // no SMB session, keystore or file operation is published by either.
        framed(&setup,|c| {
            let held=conn.receive(c).unwrap();
            let actual=held.debit.origin.hash.new_attempt().received(held.frame).unwrap().into_kdf_context();
            assert_eq!(actual.as_bytes().as_slice(),decode(hash).as_slice());
            drop(held); // no reply: source fixture closes only this connection
        });
    }
}

#[test]
fn no_common_signer_gets_the_required_cmac_answer_but_not_an_active_connection_or_session_fork() {
    let mut req = original(); context(&mut req,8,&[1,0,2,0]); // only GMAC
    let answer = fixed(&req,&server(),0x80).unwrap();
    assert_eq!(&answer.bytes()[176..], &[8,0,4,0,0,0,0,0,1,0,1,0]);
    assert!(matches!(answer.submitted(),Err(Fault::NoCommonSigning)));
    for bad in [&[0,0][..], &[2,0,1,0][..]] {
        let mut p = original(); context(&mut p,8,bad);
        assert!(matches!(opening(&p),Err(Fault::Opening)));
    }
}

#[test]
fn no_unimplemented_caps_or_invented_gss_token_even_when_requested() {
    let mut req=original();
    context(&mut req,2,&[1,0,2,0]); // offers AES-GCM; no server encryption implementation
    context(&mut req,3,&[1,0,0,0,0,0,0,0,1,0]); // compression
    context(&mut req,7,&[1,0,0,0,0,0,0,0,1,0]); // RDMA
    context(&mut req,6,&[1,0,0,0]); // transport security is not negotiated on bare TCP
    let tx=fixed(&req,&server(),0x80).unwrap();
    assert_eq!(tx.bytes(),decode(DEFAULT_WIRE).as_slice()); // one context, empty GSS, only LARGE_MTU
    let c=tx.submitted().unwrap();
    assert_eq!((c.limits.transact,c.limits.read,c.limits.write),(65_536,65_536,65_536));
}

#[test]
fn same_running_guid_different_draws_and_no_credential_or_client_fallback_on_local_failure() {
    let req=original(); let source=server(); let calls=Cell::new(0u8);
    let draw=|salt: &mut [u8;32]| { calls.set(calls.get()+1); salt.fill(calls.get()); Ok(()) };
    let a=opening(&req).unwrap().respond_from(&source,draw,local_time).unwrap();
    let b=opening(&req).unwrap().respond_from(&source,draw,local_time).unwrap();
    assert_eq!(calls.get(),2); assert_eq!(&a.bytes()[72..88],&b.bytes()[72..88]);
    assert_ne!(&a.bytes()[72..88],&req[76..92]); // never borrowed from client
    assert_ne!(&a.bytes()[142..174],&b.bytes()[142..174]);
    let other=RunningServerGuid { wire: [0x33;16] }; // another synthetic runtime, not installed data
    assert_ne!(&a.bytes()[72..88],&fixed(&req,&other,1).unwrap().bytes()[72..88]);
    assert!(matches!(opening(&req).unwrap().respond_from(&RunningServerGuid{wire:[0;16]},
        |_| panic!("invalid identity must not consume entropy"),local_time),Err(Fault::Identity)));
    assert!(matches!(opening(&req).unwrap().respond_from(&source,|_|Err(Fault::Entropy),
        ||panic!("no fabricated reply timestamp")),Err(Fault::Entropy)));
}

#[test]
fn checked_filetime_and_specific_offer_refusals() {
    let epoch=116_444_736_000_000_000;
    assert_eq!(filetime_delta(Ok(Duration::ZERO)),Some(epoch));
    assert_eq!(filetime_delta(Ok(Duration::from_nanos(199))),Some(epoch+1));
    assert_eq!(filetime_delta(Err(Duration::from_nanos(1))),Some(epoch-1));
    assert_eq!(filetime_delta(Err(Duration::from_secs(11_644_473_600))),Some(0));
    assert_eq!(filetime_delta(Err(Duration::from_secs(11_644_473_601))),None);
    assert_eq!(filetime_delta(Ok(Duration::MAX)),None);
    let mut req=original();req[100..102].copy_from_slice(&0x0302u16.to_le_bytes());
    assert!(matches!(opening(&req),Err(Fault::NoDialect)));
    let mut req=original();req[116..118].copy_from_slice(&2u16.to_le_bytes());
    assert!(matches!(opening(&req),Err(Fault::NoPreauthHash)));
    let mut req=original();req[48]=1;
    assert!(matches!(opening(&req),Err(Fault::Opening)));
}
