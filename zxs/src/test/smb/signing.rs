//! Public, synthetic fixtures only. No GSS token, real credential, SMB client
//! or running service. Independent reference data: Python hashlib SHA-512
//! chaining, hmac.digest(key, counter || label-with-NUL || NUL || hash || L,
//! "sha256")[:16], cryptography.hazmat.primitives.cmac.CMAC(AES(derived_key)).
//! For each CMAC the SIGNED flag is set and header[48..64] contains zeros.
//! No Rust implementation or response parser supplied the expected values.
use super::*;
use crate::smb::wire::{
    direct::Frame, negotiate::{CmacChoice, Offer}, preauth::{NegotiateRequestHash, tests::synthetic_exchange},
};

const TREE: &str = concat!(
    "fe534d4240000100000000000300010008000000000000000300000000000000",
    "00000000000000002a0000000000000000000000000000000000000000000000",
    "0900000048001a005c005c0066006900780074007500720065005c005a005800",
    "5300",
);
const FINAL: &str = concat!(
    "fe534d4240000100000000000100010009000000000000000200000000000000",
    "00000000000000002a0000000000000000000000000000000000000000000000",
    "090000004a000300c1c2a10100",
);
const CREATE: &str = concat!(
    "fe534d4240000100000000000500010008000000800000000400000000000000",
    "00000000070000002a0000000000000000000000000000000000000000000000",
    "3900000002000000000000000000000000000000000000008000020000000000",
    "07000000010000000000000000000000000000000000000000f1f2f3f4f5f6f7",
);
const RELATED_QUERY: &str = concat!(
    "fe534d424000010000000000100001000c000000000000000500000000000000",
    "00000000ffffffffffffffffffffffff00000000000000000000000000000000",
    "290001120000010000000000000000000000000000000000ffffffffffffffff",
    "ffffffffffffffff00",
);
// [KDF key, signed final, first ordinary, compound first, compound second]
const EXPECT: [[&str; 5]; 2] = [
    ["4e9b7b76de5b808dc8d1532494e50e95", "ac22bfc146bbb119af4ef54045b75e0d", "315c7cb02397f8363a10d5a5c9bb2f81",
     "a31665b09716237fd55bf20105958cf4", "cb387dcd1a78e94b26a29a755289af94"],
    ["ec2dbbb8b8afc943d7451f5af9c1e81e", "1feb234693b0aca47949b6ad1f19abc5", "d6776bbea66a03da20e00e79c8003088",
     "7a0d8eca9ea9703c8529cf2423e61da1", "a190ca2e83edc1e1eed500717c7dbd10"],
];

impl VerifiedExchange {
    // Low-level invalid-template checks only; the real proof supplies its own template.
    fn sign_final(self,reply:&mut [u8])->Result<SessionSigner,Fault>{self.complete(reply).map(|k|k.signer)}
}
fn hex(s: &str) -> Vec<u8> {
    s.as_bytes().chunks_exact(2).map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap()).collect()
}

#[test]
fn signing_mac_matches_rfc4493_at_every_buffer_split() {
    // RFC 4493 section 4 (https://www.rfc-editor.org/rfc/rfc4493#section-4).
    // Check the actual session MAC against published answers, not itself.
    let signer = SessionSigner {
        id: NonZeroU64::new(1).unwrap(),
        key: Zeroizing::new(hex("2b7e151628aed2a6abf7158809cf4f3c").try_into().unwrap()),
    };
    let input = hex(concat!(
        "6bc1bee22e409f96e93d7e117393172a", "ae2d8a571e03ac9c9eb76fac45af8e51",
        "30c81c46a35ce411e5fbc1191a0a52ef", "f69f2445df4f9b17ad2b417be66c3710",
    ));
    for (len, answer) in [
        (0, "bb1d6929e95937287fa37d129b756746"),
        (16, "070a16b46b4d4144f79bdd9dd04a287c"),
        (40, "dfa66747de9ae63030ca32611497c827"),
        (64, "51f0bebf7e3b9d92fc49741779363cfe"),
    ] {
        let answer = hex(answer);
        for split in 0..=len {
            let mut mac = signer.mac().unwrap();
            mac.update(&input[..split]);
            mac.update(&input[split..len]);
            assert_eq!(mac.clone().finalize().as_bytes().as_slice(), answer.as_slice(), "{len}/{split}");
            mac.verify_slice(&answer).unwrap();
        }
        let mut incorrect = answer;
        incorrect[15] ^= 1;
        let mut mac = signer.mac().unwrap();
        mac.update(&input[..len]);
        assert!(mac.verify_slice(&incorrect).is_err());
    }
}

fn envelope<T>(p: &[u8], read: impl FnOnce(&Compound<'_>) -> T) -> T {
    let mut transport = (p.len() as u32).to_be_bytes().to_vec();
    transport.extend_from_slice(p);
    let (record, rest) = Frame::split(&transport).unwrap().unwrap();
    assert!(rest.is_empty());
    read(&Compound::parse(record).unwrap())
}

fn add_context(p: &mut Vec<u8>, count_at: usize, pad: u8, alg: u16) {
    p[count_at..count_at + 2].copy_from_slice(&2u16.to_le_bytes());
    p.resize(p.len().next_multiple_of(8), pad);
    p.extend_from_slice(&[8, 0, 4, 0, 0, 0, 0, 0, 1, 0]);
    p.extend_from_slice(&alg.to_le_bytes());
}

fn exchange(explicit: bool, id: u64) -> VerifiedExchange {
    let [mut nr, mut ns, a, mut more, mut b] = synthetic_exchange();
    more[40..48].copy_from_slice(&id.to_le_bytes());
    b[40..48].copy_from_slice(&id.to_le_bytes());
    if explicit { add_context(&mut nr, 96, 0xa5, 1); add_context(&mut ns, 70, 0xb6, 1); }
    let conn = envelope(&nr, |c| {
        let offer = Offer::parse(c).unwrap();
        assert_eq!(offer.cmac_choice().unwrap(), if explicit { CmacChoice::Offered } else { CmacChoice::Absent });
        NegotiateRequestHash::received(&offer).unwrap().reply(&ns).unwrap()
    });
    let first = envelope(&a, |c| conn.new_attempt().received(c).unwrap());
    let next = first.more(&more).unwrap();
    let last = envelope(&b, |c| next.received(c).unwrap());
    // This unbound construction lives only in cfg(test), never in the connection owner.
    VerifiedExchange { request: last.into_kdf_context(), exported: Zeroizing::new(std::array::from_fn(|i| i as u8)) }
}

fn final_reply(id: u64) -> Vec<u8> {
    let mut p = hex(FINAL);
    p[40..48].copy_from_slice(&id.to_le_bytes());
    p[16..20].copy_from_slice(&1u32.to_le_bytes());
    p
}

fn verify_ordinary<'s>(frame:&Compound<'_>,lookup:impl FnMut(NonZeroU64)->Option<&'s SessionSigner>)->Result<(),Fault> {
    verify_ids(frame,lookup).map(drop).map_err(|(_,why)|why)
}
fn verify(p: &[u8], signer: &SessionSigner) -> Result<(), Fault> {
    envelope(p, |c| verify_ordinary(c, |id| (id == signer.id).then_some(signer)))
}

#[test]
fn default_and_explicit_cmac_both_require_signed_first_ordinary_command() {
    for (i, explicit) in [false, true].into_iter().enumerate() {
        let verified = exchange(explicit, 42);
        assert_eq!(&verified.signing_key().unwrap()[..], hex(EXPECT[i][0]).as_slice());
        let mut reply = final_reply(42);
        let signer = verified.sign_final(&mut reply).unwrap();
        assert_eq!(&reply[16..20], &9u32.to_le_bytes());
        assert_eq!(&reply[48..64], hex(EXPECT[i][1]).as_slice());

        let mut first = hex(TREE);
        first[48..64].copy_from_slice(&hex(EXPECT[i][2]));
        assert_eq!(verify(&first, &signer), Ok(()));
        first[16] = 0; // correct previous signature does not excuse a cleared flag
        assert_eq!(verify(&first, &signer), Err(Fault::SignatureRequired));
        first[16] = 8;
        first[48..64].copy_from_slice(&hex(EXPECT[1 - i][2])); // same public NTLM key, wrong transcript
        assert_eq!(verify(&first, &signer), Err(Fault::BadSignature));
        first[48..64].fill(0);
        assert_eq!(verify(&first, &signer), Err(Fault::BadSignature));
    }
}

#[test]
fn negotiated_selection_is_bound_to_actual_offer_and_serialized_response() {
    let [mut nr, mut ns, ..] = synthetic_exchange();
    let absent = nr.clone();
    let no_context_response = ns.clone();
    add_context(&mut nr, 96, 0xa5, 0xbeef);
    add_context(&mut ns, 70, 0xb6, 1);
    envelope(&nr, |c| {
        let offer = Offer::parse(c).unwrap();
        assert_eq!(offer.cmac_choice().unwrap(), CmacChoice::NoCommon);
        assert!(NegotiateRequestHash::received(&offer).unwrap().reply(&ns).is_ok());
        assert!(NegotiateRequestHash::received(&offer).unwrap().reply(&no_context_response).is_err());
        let mut wrong_id = ns.clone();
        *wrong_id.last_mut().unwrap() = 1;
        assert!(NegotiateRequestHash::received(&offer).unwrap().reply(&wrong_id).is_err());
    });
    envelope(&absent, |c| assert!(NegotiateRequestHash::received(&Offer::parse(c).unwrap()).unwrap().reply(&ns).is_err()));
    let mut signing_optional = no_context_response.clone();
    signing_optional[66] = 1; // our server must actually require signing
    envelope(&absent, |c| assert!(NegotiateRequestHash::received(&Offer::parse(c).unwrap()).unwrap().reply(&signing_optional).is_err()));
    for declared in [0, 2] { // empty or two IDs counted but just one present
        nr[136..138].copy_from_slice(&u16::to_le_bytes(declared));
        envelope(&nr, |c| assert!(NegotiateRequestHash::received(&Offer::parse(c).unwrap()).is_err()));
    }
}

#[test]
fn every_member_uses_raw_inherited_ids_and_its_own_trailing_padding() {
    for (i, explicit) in [false, true].into_iter().enumerate() {
        let signer = exchange(explicit, 42).sign_final(&mut final_reply(42)).unwrap();
        let mut a = hex(CREATE);
        let mut b = hex(RELATED_QUERY);
        a[48..64].copy_from_slice(&hex(EXPECT[i][3]));
        b[48..64].copy_from_slice(&hex(EXPECT[i][4]));
        a.extend_from_slice(&b);
        assert_eq!(verify(&a, &signer), Ok(()));
        a[127] ^= 1; // first member pad immediately before next header
        assert_eq!(verify(&a, &signer), Err(Fault::BadSignature));
        a[127] ^= 1;
        a[128 + 40..128 + 48].copy_from_slice(&42u64.to_le_bytes());
        assert_eq!(verify(&a, &signer), Err(Fault::BadSignature));
        a[128 + 40..128 + 48].fill(0xff);
        a[128 + 48] ^= 1;
        assert_eq!(verify(&a, &signer), Err(Fault::BadSignature));
        a[128 + 16] = 4; // unsigned second member cannot follow signed CREATE
        assert_eq!(verify(&a, &signer), Err(Fault::SignatureRequired));
    }
}

#[test]
fn unrelated_compound_resolves_two_distinct_live_sessions_and_both_signatures() {
    let first = exchange(false, 42).sign_final(&mut final_reply(42)).unwrap();
    let second = exchange(false, 43).sign_final(&mut final_reply(43)).unwrap();
    let mut a = hex(TREE);
    a.resize(104, 0xac);
    a[20..24].copy_from_slice(&104u32.to_le_bytes());
    a[48..64].copy_from_slice(&hex("5714150158c372e650fe539c212826d3"));
    let mut b = hex(TREE);
    b[24] = 4; b[40] = 43;
    b[48..64].copy_from_slice(&hex("a73044f52039ff8f23f0f6a2a7e4416f"));
    a.extend_from_slice(&b);
    envelope(&a, |c| {
        assert_eq!(verify_ordinary(c, |id| match id.get() { 42 => Some(&first), 43 => Some(&second), _ => None }), Ok(()));
        assert_eq!(verify_ordinary(c, |_| Some(&first)), Err(Fault::UnknownSession));
        assert_eq!(verify_ordinary(c, |id| (id == first.id).then_some(&first)), Err(Fault::UnknownSession));
    });
}

#[test]
fn invalid_final_response_is_not_mutated_and_wrong_phase_does_not_verify() {
    for (at, value) in [(6, 2), (8, 1), (12, 3), (16, 9), (16, 0x21), (24, 9), (32, 1), (36, 1),
                        (40, 43), (48, 5), (66, 1), (66, 2), (66, 4), (70, 100)] {
        let mut final_bytes = final_reply(42);
        final_bytes[at] = value;
        let original = final_bytes.clone();
        assert!(matches!(exchange(false, 42).sign_final(&mut final_bytes), Err(Fault::InvalidFinalResponse)));
        assert_eq!(final_bytes, original);
    }
    let signer = exchange(false, 42).sign_final(&mut final_reply(42)).unwrap();
    for cmd in [0, 1, 12, 255] {
        let mut p = hex(TREE);
        p[12] = cmd;
        assert_eq!(verify(&p, &signer), Err(Fault::WrongPhase));
    }
}
