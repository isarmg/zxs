//! Fixed synthetic wire bytes ONLY. The opaque token markers are not GSS
//! tokens or user credentials. Data source for the expected hashes: independent
//! Python standard library, starting with bytes(64) and for each of the following
//! messages in order taking hashlib.sha512(previous + bytes.fromhex(message)).digest().
//! No Rust implementation supplied the answers. These are not interoperability results.
use super::*;
use crate::smb::wire::direct::Frame;
const NEG_REQ: &str = concat!(
    "fe534d4240000100000000000000010000000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "240001000300000004000000c0c1c2c3c4c5c6c7c8c9cacbcccdcecf68000000",
    "010000001103717201000a000000000001000400010001020304",
);
const NEG_RSP: &str = concat!(
    "fe534d4240000100000000000000010001000000000000000000000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "4100030011030100a0a1a2a3a4a5a6a7a8a9aaabacadaeaf0400000000000100",
    "0000010000000100000000000000000000000000000000008000030088000000",
    "6001008081828384010026000000000001002000010000010203040506070809",
    "0a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
);
const SETUP_1: &str = concat!(
    "fe534d4240000100000000000100010000000000000000000100000000000000",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "1900000200000000000000005a0003000000000000000000a1a2610100",
);
const CONTINUE: &str = concat!(
    "fe534d4240000100160000c00100010001000000000000000100000000000000",
    "00000000000000002a0000000000000000000000000000000000000000000000",
    "090000004a000300b1b2620100",
);
const SETUP_2: &str = concat!(
    "fe534d4240000100000000000100010000000000000000000200000000000000",
    "00000000000000002a0000000000000000000000000000000000000000000000",
    "1900000200000000000000005c0005000000000000000000d1d2d3d4630300fe",
    "ff",
);
const HASHES: [&str; 5] = [
    concat!("809818d2db2d83b39b5d9e1d6170be02f816f348b43d9a7d01bc0edd2698127c",
            "b4e41d97c02e4a089875be26fda761e76aa44215ebcbbfb1c6cce732bb9e0957"),
    concat!("5e6639d0d773fa001cb3f502527d21e734be2b542f86265efdac6ec2d3a05e1c",
            "e9b159aa50e382a91602f3f9bf0e95aac432dbaf79754ef4f67cdb47fe906e9b"),
    concat!("16ae1fdbe31d8b55219fdfe078967e2d68a4cf710d0e83eb91c8d166d6466276",
            "ac30fe41ad180d96278e379af8463cab6a841abe9e7988489a864298e422d19c"),
    concat!("669c7e3ced2e8929238362826500686f50f7b006c353e893b8ba3b1f9879b1dc",
            "955884a2bf0020174710216bfa3ebd61fe0118625b2816e957306bd2fd805142"),
    concat!("5c410119f80640fd25c1b136ab9160b40be6fa955369d2e81ca8a68c03444a7b",
            "56a7938a1383af15772c4d81e62b32a4816b4aac3b4a1dfea73752f4a4731215"),
];

fn hex(s: &str) -> Vec<u8> {
    s.as_bytes().chunks_exact(2).map(|p| {
        u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap()
    }).collect()
}

pub(in crate::smb) fn synthetic_exchange() -> [Vec<u8>; 5] {
    [NEG_REQ, NEG_RSP, SETUP_1, CONTINUE, SETUP_2].map(hex)
}

fn expected(i: usize) -> Hash { hex(HASHES[i]).try_into().unwrap() }

fn envelope<T>(p: &[u8], read: impl FnOnce(&Compound<'_>) -> T) -> T {
    let mut transport = (p.len() as u32).to_be_bytes().to_vec();
    transport.extend_from_slice(p);
    let (frame, rest) = Frame::split(&transport).unwrap().unwrap();
    assert!(rest.is_empty());
    let c = Compound::parse(frame).unwrap();
    read(&c)
}

fn negotiate() -> NegotiateRequestHash {
    envelope(&hex(NEG_REQ), |c| NegotiateRequestHash::received(&Offer::parse(c).unwrap()).unwrap())
}

fn ready() -> ConnectionHash { negotiate().reply(&hex(NEG_RSP)).unwrap() }

fn first() -> SetupRequestHash {
    envelope(&hex(SETUP_1), |c| ready().new_attempt().received(c).unwrap())
}

#[test]
fn all_five_original_wire_messages_match_independent_fixed_hashes() {
    let req = negotiate();
    assert_eq!(req.value, expected(0));
    let base = req.reply(&hex(NEG_RSP)).unwrap();
    assert_eq!(base.0, expected(1));
    let first = envelope(&hex(SETUP_1), |c| base.new_attempt().received(c).unwrap());
    assert_eq!(first.value, expected(2));
    let more = first.more(&hex(CONTINUE)).unwrap();
    assert_eq!(more.value, expected(3));
    let last = envelope(&hex(SETUP_2), |c| more.received(c).unwrap());
    assert_eq!(last.into_kdf_context().as_bytes(), &expected(4));
    assert_eq!(base.0, expected(1)); // per-session steps don't mutate the connection fork point
}

#[test]
fn preserved_alignment_and_separate_new_attempts() {
    let base = ready();
    let mut modified = hex(SETUP_1);
    modified[88] ^= 1; // gap before opaque security token, outside its declared length
    let other = envelope(&modified, |c| base.new_attempt().received(c).unwrap());
    assert_ne!(other.into_kdf_context().as_bytes(), &expected(2));
    let original = envelope(&hex(SETUP_1), |c| base.new_attempt().received(c).unwrap());
    assert_eq!(original.into_kdf_context().as_bytes(), &expected(2));
    let mut prefixed = (hex(NEG_RSP).len() as u32).to_be_bytes().to_vec();
    prefixed.extend(hex(NEG_RSP));
    assert!(negotiate().reply(&prefixed).is_err()); // TCP framing is never a hash input
}

#[test]
fn no_cross_session_message_or_final_reply_in_request_side_key_input() {
    let mut response = hex(CONTINUE);
    response[8..12].fill(0); // final success MUST NOT be extended into the KDF context
    assert!(first().more(&response).is_err());
    let mut response = hex(CONTINUE);
    response[24] = 2;
    assert!(first().more(&response).is_err()); // wrong MessageId
    response[24] = 1;
    response[40..48].fill(0xff);
    assert!(first().more(&response).is_err());
    let mut second = hex(SETUP_2);
    second[40] = 43;
    let pending = first().more(&hex(CONTINUE)).unwrap();
    assert!(envelope(&second, |c| pending.received(c)).is_err());
    let mut bound = hex(SETUP_1);
    bound[66] = 1;
    assert!(envelope(&bound, |c| ready().new_attempt().received(c)).is_err());
}

#[test]
fn default_and_explicit_signing_offers_do_not_disable_signatures() {
    envelope(&hex(NEG_REQ), |c| {
        let offer = Offer::parse(c).unwrap();
        assert!(offer.signing_algorithms().unwrap().is_none());
    });
    let mut explicit = hex(NEG_REQ);
    explicit[96..98].copy_from_slice(&2u16.to_le_bytes());
    explicit.resize(128, 0xa5);
    explicit.extend_from_slice(&[8,0,4,0,0,0,0,0,1,0,1,0]); // one signing ID: AES-CMAC
    envelope(&explicit, |c| {
        let offer = Offer::parse(c).unwrap();
        assert!(offer.signing_algorithms().unwrap().unwrap().contains(1));
        assert_ne!(NegotiateRequestHash::received(&offer).unwrap().value, expected(0));
    });
    // The two actual first-command MAC cases are in src/test/smb/signing.rs.
}

#[test]
fn refused_mismatched_hash_response_and_token_boundaries() {
    let mut r = hex(NEG_RSP);
    r[148] = 2;
    assert!(negotiate().reply(&r).is_err());
    let mut r = hex(NEG_RSP);
    r[124..128].copy_from_slice(&128u32.to_le_bytes()); // overlaps declared token
    assert!(negotiate().reply(&r).is_err());
    let mut first_request = hex(SETUP_1);
    first_request[78..80].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(envelope(&first_request, |c| ready().new_attempt().received(c)).is_err());
}
