//! Publicly invented, complete NTLMv2 fixture messages. No account/password,
//! real challenge, network capture, responder, or on-the-fly proof generator.
//! Independent known answers: Python stdlib HMAC/MD5 and cryptography ARC4.
//! The full development messages use the public 0x60..0x6f key. The good
//! messages and policy-negative, cryptographically valid alternatives are fixed.
use super::*;
use crate::smb::session::envelope::tests as frame;
use std::time::Duration;

const PLAIN_ONE: &str = "4e544c4d535350000100000015820822000000000000000000000000000000000a00654a0000000f";
const PLAIN_TWO: &str = concat!(
    "4e544c4d5353500002000000060006003800000015828a221112131415161718000000000000000028002800",
    "3e0000000a00654a0000000f42004f0058000100060042004f00580002000a00530059004e00540048000700",
    "08004433221100bcda0100000000",
);
const PLAIN_THREE: &str = concat!(
    "4e544c4d53535000030000000000000000000000a000a000580000000a000a00f80000001600160002010000",
    "0e000e0018010000000000000000000015828a220a00654a0000000f88c86b1c805c4e011202530882d1c64f",
    "e0c8d30633b413bb653814a24cff78b901010000000000004433221100bcda01414243444546474800000000",
    "0100060042004f00580002000a00530059004e0054004800070008004433221100bcda010600040002000000",
    "0900280063006900660073002f0066006900780074007500720065002e0069006e00760061006c0069006400",
    "0a001000000000000000000000000000000000000000000000000000530059004e0054004800570069007200",
    "650046006900780074007500720065004600490058005400550052004500",
);
const PLAIN_EXPORTED: &str = "f7bcf45a167d64d4096548daf3c0595a";
const PLAIN_CLIENT: &str = "01000000422a93d04854535e00000000";
const PLAIN_SERVER: &str = "010000001d5393a25f19cafb00000000";
const EXCH_ONE: &str = "4e544c4d535350000100000015820862000000000000000000000000000000000a00654a0000000f";
const EXCH_TWO: &str = concat!(
    "4e544c4d5353500002000000060006003800000015828a623132333435363738000000000000000028002800",
    "3e0000000a00654a0000000f42004f0058000100060042004f00580002000a00530059004e00540048000700",
    "08004433221100bcda0100000000",
);
const EXCH_THREE: &str = concat!(
    "4e544c4d53535000030000000000000000000000a000a000580000000a000a00f80000001600160002010000",
    "0e000e0028010000100010001801000015828a620a00654a0000000fe334945aa0139f440a7bf13d82c5b6f9",
    "7e3f0a00c5b62107202f2623f6c306d701010000000000004433221100bcda01414243444546474800000000",
    "0100060042004f00580002000a00530059004e0054004800070008004433221100bcda010600040002000000",
    "0900280063006900660073002f0066006900780074007500720065002e0069006e00760061006c0069006400",
    "0a001000000000000000000000000000000000000000000000000000530059004e0054004800570069007200",
    "650046006900780074007500720065003cf6caf24f2d6f8d10b1992a1d844383460049005800540055005200",
    "4500",
);
const EXCH_EXPORTED: &str = "909192939495969798999a9b9c9d9e9f";
const EXCH_CLIENT: &str = "010000002cb5b95579b9e55b00000000";
const EXCH_SERVER: &str = "0100000062c761def508a6f100000000";
const NO_INNER: &str = concat!(
    "4e544c4d5353500003000000000000000000000098009800480000000a000a00e000000016001600ea000000",
    "0e000e0000010000000000000000000015828a220a00654a0000000f63a621eb85ce335b6ace5fb54e5d0c04",
    "01010000000000004433221100bcda014142434445464748000000000100060042004f00580002000a005300",
    "59004e0054004800070008004433221100bcda010900280063006900660073002f0066006900780074007500",
    "720065002e0069006e00760061006c00690064000a0010000000000000000000000000000000000000000000",
    "00000000530059004e0054004800570069007200650046006900780074007500720065004600490058005400",
    "550052004500",
);
const WRONG_SPN: &str = concat!(
    "4e544c4d535350000300000000000000000000009c009c00580000000a000a00f400000016001600fe000000",
    "0e000e0014010000000000000000000015828a220a00654a0000000fd1b5a791dc94f1c8db39acc4a34147f0",
    "9f04881f26033b30bb1051427745ab6a01010000000000004433221100bcda01414243444546474800000000",
    "0100060042004f00580002000a00530059004e0054004800070008004433221100bcda010600040002000000",
    "0900240063006900660073002f006f0074006800650072002e0069006e00760061006c00690064000a001000",
    "000000000000000000000000000000000000000000000000530059004e005400480057006900720065004600",
    "6900780074007500720065004600490058005400550052004500",
);
const WRONG_TIME: &str = concat!(
    "4e544c4d53535000030000000000000000000000a000a000580000000a000a00f80000001600160002010000",
    "0e000e0018010000000000000000000015828a220a00654a0000000f97e45dce1f3aab6f3746b7553e0dcf4c",
    "929a1517a8b758638ca9652b8463181801010000000000004533221100bcda01414243444546474800000000",
    "0100060042004f00580002000a00530059004e0054004800070008004433221100bcda010600040002000000",
    "0900280063006900660073002f0066006900780074007500720065002e0069006e00760061006c0069006400",
    "0a001000000000000000000000000000000000000000000000000000530059004e0054004800570069007200",
    "650046006900780074007500720065004600490058005400550052004500",
);

fn hex(s: &str) -> Vec<u8> {
    s.as_bytes().chunks_exact(2).map(|x|
        u8::from_str_radix(std::str::from_utf8(x).unwrap(),16).unwrap()).collect()
}
fn utf(s: &str) -> Box<[u8]> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>().into_boxed_slice()
}
pub(super) fn revision() -> Arc<ProtectedRevision> {
    Arc::new(ProtectedRevision {
        user: utf("WireFixture"), domain: utf("SYNTH"), spn: utf("cifs/fixture.invalid"),
        epoch: NonZeroU64::new(17).unwrap(),
        response_key: Zeroizing::new(std::array::from_fn(|i| 0x60 + i as u8)),
    })
}
pub(super) fn challenge_vector()->(Vec<u8>,Vec<u8>) {(hex(PLAIN_ONE),hex(PLAIN_TWO))}
struct Example { one: Vec<u8>, two: Vec<u8>, three: Vec<u8>, exported: Vec<u8>,
                 client_list: Vec<u8>, server_list: Vec<u8> }
fn example(exch: bool) -> Example {
    let [a,b,c,k,i,o] = if exch {
        [EXCH_ONE,EXCH_TWO,EXCH_THREE,EXCH_EXPORTED,EXCH_CLIENT,EXCH_SERVER]
    } else {
        [PLAIN_ONE,PLAIN_TWO,PLAIN_THREE,PLAIN_EXPORTED,PLAIN_CLIENT,PLAIN_SERVER]
    };
    Example { one: hex(a), two: hex(b), three: hex(c), exported: hex(k),
              client_list: hex(i), server_list: hex(o) }
}
// None = direct NTLM; wrapped = NTLM alone or followed by offered Kerberos.
fn issued(revision: &Arc<ProtectedRevision>, e: &Example, wrapped: Option<bool>) -> Issued {
    let first = match wrapped {
        None => e.one.clone(),
        Some(false) => frame::init(&[frame::NTLM], Some(&e.one)),
        Some(true) => frame::init(&[frame::NTLM,frame::KRB], Some(&e.one)),
    };
    Issued { start: frame::seen(&first), revision:Arc::clone(revision), assigned: NonZeroU64::new(42).unwrap(),
             expected_type2: Saved::from_wire(&e.two).unwrap(),
             expires: Instant::now() + Duration::from_secs(60) }
}
fn more(two: &[u8], wrapped: bool, mid: u64, sid: u64) -> Vec<u8> {
    if wrapped {
        frame::smb(&frame::resp(Some(1),true,Some(two),None),true,mid,sid,0)
    } else { frame::smb(two,true,mid,sid,0) }
}
fn done(
    revision: &Arc<ProtectedRevision>, e: &Example, wrapped: Option<bool>,
    mic: Option<&[u8]>, sid: u64, gap: u8,
) -> Result<MechanismProof> {
    let w = wrapped.is_some();
    let pending = issued(revision,e,wrapped).sent(&more(&e.two,w,1,42))?;
    let token = if w { frame::resp(None,false,Some(&e.three),mic) } else { e.three.clone() };
    let buf = frame::framed(&frame::smb(&token,false,2,sid,gap));
    pending.received(&frame::compound(&buf))
}

#[test]
fn fixed_proof_inner_mic_exported_key_and_two_directions_with_or_without_key_exchange() {
    let v = revision();
    for kx in [false,true] {
        let e = example(kx);
        let raw = done(&v,&e,None,None,42,0).unwrap();
        assert_eq!(&raw.exported[..], e.exported.as_slice());
        assert!(Arc::ptr_eq(&raw.revision,&v));
        assert_eq!(raw.revision.epoch.get(),17);
        assert!(matches!(raw.final_gss,FinalGss::Direct));
        let wrapped = done(&v,&e,Some(true),Some(&e.client_list),42,0).unwrap();
        assert_eq!(&wrapped.exported[..],e.exported.as_slice());
        let FinalGss::Spnego(out) = wrapped.final_gss else { panic!("must send server list MIC"); };
        assert_eq!(out.as_slice(),frame::resp(Some(0),false,None,Some(&e.server_list)).as_slice());
        // Same NTLM key and messages do not imply the same SMB transcript.
        assert_ne!(raw.request.as_bytes(),wrapped.request.as_bytes());
        let change = done(&v,&e,Some(true),Some(&e.client_list),42,0xee).unwrap();
        assert_ne!(wrapped.request.as_bytes(),change.request.as_bytes());
        assert_eq!(&change.exported[..],e.exported.as_slice());
    }
}

#[test]
fn assembled_der_raw_ntlm_and_outer_smb_each_keep_their_original_scope() {
    let v = revision();
    for kx in [false,true] {
        let e = example(kx);
        // Correct NT proof + inner MIC, missing, wrong direction, different
        // valid offered list, wrong sequence or damaged outer checksum.
        assert!(done(&v,&e,Some(true),None,42,0).is_err());
        assert!(done(&v,&e,Some(true),Some(&e.server_list),42,0).is_err());
        assert!(done(&v,&e,Some(false),Some(&e.client_list),42,0).is_err());
        for at in [0,4,12] {
            let mut tag = e.client_list.clone(); tag[at] ^= 1;
            assert!(done(&v,&e,Some(true),Some(&tag),42,0).is_err());
        }
        assert!(done(&v,&e,None,None,43,0).is_err());
        let mut wrong = e.two.clone(); wrong[24] ^= 1;
        assert!(issued(&v,&e,None).sent(&more(&wrong,false,1,42)).is_err());
        assert!(issued(&v,&e,None).sent(&more(&e.two,false,3,42)).is_err());
        assert!(issued(&v,&e,None).sent(&more(&e.two,false,1,43)).is_err());
    }
}

#[test]
fn valid_proof_is_not_enough_without_strict_mic_original_inputs_and_bound_target() {
    let v = revision();
    // Unlike simply flipping a random packet byte, all three alternate NTLMv2
    // proofs match this public verifier. Wrong SPN/time also have valid
    // inner MICs under their own derived key. The strict policy still denies.
    for sample in [NO_INNER,WRONG_SPN,WRONG_TIME] {
        let mut e = example(false); e.three = hex(sample);
        assert!(done(&v,&e,None,None,42,0).is_err());
    }
    for at in [0,1,2,3,4] {
        let mut e = example(true);
        let proof_start = u32::from_le_bytes(e.three[24..28].try_into().unwrap()) as usize;
        let key_start = u32::from_le_bytes(e.three[56..60].try_into().unwrap()) as usize;
        match at {
            0 => e.one[32] ^= 1, // only original Type1 VERSION bytes changed
            1 => e.two[48] ^= 1, // actual registered Type2, same server nonce
            2 => e.three[72] ^= 1, // correct NTProofStr, wrong inner MIC
            3 => e.three[proof_start] ^= 1,
            _ => e.three[key_start] ^= 1,
        }
        assert!(done(&v,&e,None,None,42,0).is_err());
    }
    let e = example(false);
    for mismatch in [0,1,2,3] {
        let mut other = revision();
        let slot=Arc::get_mut(&mut other).unwrap();
        match mismatch {
            0 => slot.response_key[0] ^= 1,
            1 => slot.domain = utf("OTHER"),
            2 => slot.user = utf("OtherFixture"),
            _ => slot.spn = utf("cifs/other.invalid"),
        }
        assert!(done(&other,&e,None,None,42,0).is_err());
    }
}

#[test]
fn consumed_challenge_stages_reject_expiry_reserved_sid_and_changed_security_flags() {
    let v = revision();
    let e = example(false);
    let mut stale = issued(&v,&e,None); stale.expires = Instant::now();
    assert!(stale.sent(&more(&e.two,false,1,42)).is_err());
    let mut wrong = issued(&v,&e,None); wrong.assigned = NonZeroU64::MAX;
    assert!(wrong.sent(&more(&e.two,false,1,u64::MAX)).is_err());
    let mut p = issued(&v,&e,None).sent(&more(&e.two,false,1,42)).unwrap();
    p.expires = Instant::now();
    let b = frame::framed(&frame::smb(&e.three,false,2,42,0));
    assert!(p.received(&frame::compound(&b)).is_err());
    let s = MIN | TARGET_INFO;
    assert!(selected(MIN,s,s).is_ok());
    for bit in [VERSION, SIGN, ESS, BITS128] {
        assert!(selected(MIN & !bit,s,s).is_err());
        assert!(selected(MIN,s & !bit,s & !bit).is_err());
        assert!(selected(MIN,s,s & !bit).is_err());
    }
    assert!(selected(MIN,s|KEY_EXCH,s|KEY_EXCH).is_err());
    assert!(selected(MIN|KEY_EXCH,s|KEY_EXCH,s).is_err());
    assert!(selected(MIN,s|0x80,s|0x80).is_err());
}

#[test]
fn microsoft_section_4_2_4_public_session_base_key() {
    // MS-NLMP §§4.2.4.1.1, 4.2.4.2.2, 4.2.4.1.2, respectively. No
    // password, identity or promotion of the vendor example to a live login.
    let response_key = hex("0c868a403bfd7a93a3001ef22ef02e3f");
    let nt_proof = hex("68cd0ab851e51c96aabc927bebef6a1c");
    let base = finish_hmac(hmac(&response_key,&[&nt_proof]).unwrap());
    assert_eq!(&base[..],hex("8de40ccadbc14a82f15cb0ad0de95ca3").as_slice());
}

impl challenge::ChallengeService {
    /// Only public fixed bytes, never an NTLM response generator or a key API.
    pub(in crate::smb) fn fixture_messages(wrapped:bool)->(Vec<u8>,Vec<u8>) {
        let e=example(false);
        if wrapped {(frame::init(&[frame::NTLM,frame::KRB],Some(&e.one)),
                     frame::resp(None,false,Some(&e.three),Some(&e.client_list)))}
        else {(e.one,e.three)}
    }
}

impl challenge::ChallengeService {
    /// The same exact public list and three original NTLM fixture messages,
    /// without an optimistic first token. No secret, generator or service.
    pub(in crate::smb) fn fixture_selected_messages()->(Vec<u8>,Vec<u8>,Vec<u8>) {
        let e=example(false);
        (frame::init(&[frame::NTLM,frame::KRB],None),frame::resp(None,false,Some(&e.one),None),
         frame::resp(None,false,Some(&e.three),Some(&e.client_list)))
    }
}
