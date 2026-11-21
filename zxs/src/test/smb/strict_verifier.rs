//! Publicly invented, complete NTLMv2 fixture messages. No account/password,
//! real challenge, network capture, responder, or on-the-fly proof generator.
//! Independent known answers: Python stdlib HMAC/MD5 and cryptography ARC4.
//! The full development messages use the public 0x60..0x6f key. The good
//! messages and policy-negative, cryptographically valid alternatives are fixed.
use super::*;
use crate::smb::session::envelope::tests as frame;
use std::time::Duration;

pub(super) const FIXTURE_SERVER_TIME:u64=0x01da_bc00_1122_3344;

#[test]
fn private_key_exchange_source_keeps_its_known_stream_and_erasure_trait() {
    // RFC 6229 §2, key 0x0102030405, offset 0. The production verifier's
    // NTLM/SPNEGO known answers below also remain unmodified.
    let mut first = [0u8; 16];
    rc4_apply(&[1, 2, 3, 4, 5], &mut first).unwrap();
    assert_eq!(first, [0xb2, 0x39, 0x63, 0x05, 0xf0, 0x3d, 0xc0, 0x27,
                       0xcc, 0xc3, 0x52, 0x4a, 0x0a, 0x11, 0x18, 0xa8]);
    fn requires_erasure<T: cipher::zeroize::ZeroizeOnDrop>() {}
    requires_erasure::<Rc4>();
}

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
const INDEPENDENT_HEADER_TIME: &str = concat!(
    "4e544c4d53535000030000000000000000000000a000a000580000000a000a00f80000001600160002010000",
    "0e000e0018010000000000000000000015828a220a00654a0000000f97e45dce1f3aab6f3746b7553e0dcf4c",
    "929a1517a8b758638ca9652b8463181801010000000000004533221100bcda01414243444546474800000000",
    "0100060042004f00580002000a00530059004e0054004800070008004433221100bcda010600040002000000",
    "0900280063006900660073002f0066006900780074007500720065002e0069006e00760061006c0069006400",
    "0a001000000000000000000000000000000000000000000000000000530059004e0054004800570069007200",
    "650046006900780074007500720065004600490058005400550052004500",
);

// Public fixed proof/MIC for INDEPENDENT_HEADER_TIME, and a distinct packet
// with independently VALID HMAC/MIC but an altered server-owned TimeStamp AV.
const INDEPENDENT_KEY:&str="89d43927145bdcbc5446b3e79c185b91";
const INDEPENDENT_CLIENT:&str="01000000a31e5976d1ee6e6300000000";
const INDEPENDENT_SERVER:&str="010000006e8988daa26dab5300000000";
const ALTERED_ISSUED_AV:&str=concat!(
    "4e544c4d53535000030000000000000000000000a000a000580000000a000a00f80000001600160002010000",
    "0e000e0018010000000000000000000015828a220a00654a0000000fb7755aa2d0189c2817f6849632a14111",
    "39a78b524e97c06d1fa8cc47a4f43f1901010000000000004533221100bcda01414243444546474800000000",
    "0100060042004f00580002000a00530059004e0054004800070008004633221100bcda010600040002000000",
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
        user: utf("wirefixture"), domain: utf("SYNTH"), spn: utf("cifs/fixture.invalid"),
        epoch: NonZeroU64::new(17).unwrap(),
        response_key: Zeroizing::new(std::array::from_fn(|i| 0x60 + i as u8)),
        source: RevisionSource::Fixture,
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
    done_at(revision,e,wrapped,mic,sid,gap,FIXTURE_SERVER_TIME)
}

fn done_at(
    revision: &Arc<ProtectedRevision>, e: &Example, wrapped: Option<bool>,
    mic: Option<&[u8]>, sid: u64, gap: u8, server_time:u64,
) -> Result<MechanismProof> {
    let w = wrapped.is_some();
    let pending = issued(revision,e,wrapped).sent(&more(&e.two,w,1,42))?;
    let token = if w { frame::resp(None,false,Some(&e.three),mic) } else { e.three.clone() };
    let buf = frame::framed(&frame::smb(&token,false,2,sid,gap));
    pending.received(&frame::compound(&buf),server_time)
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
    // All three alternate NTLMv2 proofs match this public verifier. Wrong
    // SPN or altered issued AV also have valid inner MICs under their own
    // derived key. Neither a fresh header nor proof can replace those bindings.
    for sample in [NO_INNER,WRONG_SPN,ALTERED_ISSUED_AV] {
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
fn only_header_clock_may_differ_and_the_exact_ntlm_and_spnego_proofs_still_apply() {
    let v=revision();let mut e=example(false);e.three=hex(INDEPENDENT_HEADER_TIME);
    let header=FIXTURE_SERVER_TIME+1;
    let skew=1_200_000_000; // independent local 120s expectation in FILETIME ticks
    let parsed=ntlm::authenticate(&e.three).unwrap();
    assert_eq!(parsed.nt_response[24..32],header.to_le_bytes());
    assert_eq!(parsed.avs.get(7),Some(&FIXTURE_SERVER_TIME.to_le_bytes()[..]));
    let inner=done(&v,&e,None,None,42,0).unwrap();
    assert_eq!(&inner.exported[..],hex(INDEPENDENT_KEY).as_slice());
    let list=hex(INDEPENDENT_CLIENT);
    let proof=done(&v,&e,Some(true),Some(&list),42,0).unwrap();
    let FinalGss::Spnego(both)=proof.final_gss else {panic!("must verify and answer list MIC");};
    assert_eq!(both.as_slice(),frame::resp(Some(0),false,None,Some(&hex(INDEPENDENT_SERVER))).as_slice());
    assert!(done(&v,&e,Some(true),None,42,0).is_err());

    // Same authenticated header + ORIGINAL AV; alter only the verifier's
    // time. Include both signed boundaries and extremes (no time overflow).
    for now in [header-skew,header+skew] {
        assert!(done_at(&v,&e,None,None,42,0,now).is_ok());
    }
    for now in [header-skew-1,header+skew+1,0,u64::MAX] {
        assert!(done_at(&v,&e,None,None,42,0,now).is_err());
    }

    // A still-in-window but substituted header, proof or MIC cannot advance.
    for at in [88+24,88,72] {
        e.three[at]^=2;assert!(done(&v,&e,None,None,42,0).is_err());e.three[at]^=2;
    }
    let mut expired=issued(&v,&e,None).sent(&more(&e.two,false,1,42)).unwrap();
    expired.expires=Instant::now();
    let request=frame::framed(&frame::smb(&e.three,false,2,42,0));
    assert!(expired.received(&frame::compound(&request),header).is_err());
}

#[test]
fn ascii_case_matches_the_one_canonical_ntlm_identity_without_rewriting_the_message() {
    let v=revision();let expected=hex(PLAIN_EXPORTED);
    let user=258..280; // original public fixture's 11 UTF-16LE units
    // Both name spellings retain the same NTProofStr and session key. These
    // are fixed, independent HMAC answers over the changed original Type 3.
    for (spelling,mic) in [
        ("57004900520045004600490058005400550052004500","cf41a72555b3d277320da0b5e9b4d66d"),
        ("77006900720065006600690078007400750072006500","7cd68f8607dc7898163ae3ef0de091ce"),
    ] {
        let mut e=example(false);e.three[user.clone()].copy_from_slice(&hex(spelling));
        // Matching the configured account is NOT authorization to skip MIC.
        assert!(done(&v,&e,None,None,42,0).is_err());
        e.three[72..88].copy_from_slice(&hex(mic));
        let proof=done(&v,&e,None,None,42,0).unwrap();
        assert_eq!(&proof.exported[..],expected.as_slice());
        assert!(Arc::ptr_eq(&proof.revision,&v));
        assert!(done(&v,&e,Some(true),Some(&e.client_list),42,0).is_ok());
    }

    let mut e=example(false);
    e.three[user].copy_from_slice(&hex("57ff6900720065006600690078007400750072006500"));
    e.three[72..88].copy_from_slice(&hex("fc52ea72e04c6d5676ede0509da342ff"));
    // A Unicode lookalike with the same low W byte is not this ASCII account.
    assert!(done(&v,&e,None,None,42,0).is_err());
    let mut e=example(false);
    e.three[248..258].copy_from_slice(&utf("synth"));
    e.three[72..88].copy_from_slice(&hex("4f16102492c5054454487e6233c1f467"));
    assert!(done(&v,&e,None,None,42,0).is_err()); // domain bytes remain exact

    for (configured,sent) in [("",""),("WireFixture","WIREFIXTURE"),("a.b","A_B"),("a:b","A:B"),(".a",".A")] {
        assert!(!same_local_login(&utf(configured),&utf(sent)));
    }
    assert!(same_local_login(&utf("one-2._"),&utf("ONE-2._")));
    assert!(!same_local_login(&utf("a"),&[b'A']));
    assert!(!same_local_login(&utf("a"),&[b'A',0,0,0])); // no odd-width or padded aliases
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
    assert!(p.received(&frame::compound(&b),FIXTURE_SERVER_TIME).is_err());
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
