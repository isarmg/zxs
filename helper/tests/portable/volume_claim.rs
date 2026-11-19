use super::{Claim, Invalid, LEN, ROOT_SLOT, Role};
use sha2::{Digest, Sha256};

const VECTORS: &str = include_str!("volume-claim-v1.txt");

fn vector(key: &str) -> Vec<u8> {
    let raw = VECTORS.lines().filter_map(|line| line.split_once('='))
        .find(|(k, _)| *k == key).expect("fixed independent vector").1;
    assert!(raw.is_ascii() && raw.len().is_multiple_of(2));
    (0..raw.len()).step_by(2)
        .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).unwrap()).collect()
}

fn id(start: u8) -> [u8; 16] {
    std::array::from_fn(|i| start + i as u8)
}

#[test]
fn exact_bytes_agree_for_three_initial_roles_and_a_later_acquisition() {
    for (key, role, vol, op, source) in [
        ("initial_s", Role::System, 0x21, 0x81, "init"),
        ("initial_r", Role::Recycle, 0x31, 0x81, "init"),
        ("initial_f", Role::Data, 0x41, 0x81, "init"),
        ("later_f", Role::Data, 0x51, 0x91, "add"),
    ] {
        let digest: [u8; 32] = Sha256::digest(vector(&format!("source_input_{source}"))).into();
        assert_eq!(digest.as_slice(), vector(&format!("source_digest_{source}")).as_slice());
        let expected = Claim::from_fields(role, id(1), id(vol), id(op), digest).unwrap();
        let bytes = vector(key);
        assert_eq!(bytes.len(), LEN);
        assert_eq!(expected.encode().as_slice(), bytes.as_slice());
        assert_eq!(Claim::decode(&bytes), Ok(expected));
        assert_eq!(expected.check_row(&bytes), Ok(()));
    }
}

#[test]
fn missing_and_trailing_bytes_other_domains_roles_and_null_ids_are_not_claims() {
    let good = vector("initial_s");
    for len in 0..LEN {
        assert_eq!(Claim::decode(&good[..len]), Err(Invalid::Length));
    }
    let mut extra = good.clone();
    extra.push(0);
    assert_eq!(Claim::decode(&extra), Err(Invalid::Length));
    let mut old = good.clone();
    for domain in [b"ZXS/VOLUME/MAINT-1\0".as_slice(), b"ZXS/VOLUME/ID-LEAF-1\0".as_slice()] {
        old[..22].fill(0);
        old[..domain.len()].copy_from_slice(domain);
        assert_eq!(Claim::decode(&old), Err(Invalid::Domain));
    }
    let mut nul = good.clone();
    nul[21] = b'0';
    assert_eq!(Claim::decode(&nul), Err(Invalid::Domain));
    for tag in [3, b'S', b'R', b'F', 255] {
        let mut wrong = good.clone();
        wrong[22] = tag;
        assert_eq!(Claim::decode(&wrong), Err(Invalid::Role));
    }
    for start in [23, 39, 55] {
        let mut wrong = good.clone();
        wrong[start..start + 16].fill(0);
        assert_eq!(Claim::decode(&wrong), Err(Invalid::ZeroIdentity));
    }
}

#[test]
fn a_well_formed_marker_still_has_to_match_every_original_row_field() {
    let original = vector("initial_f");
    let expected = Claim::decode(&original).unwrap();
    for at in [22, 23, 38, 39, 54, 55, 70, 71, 102] {
        let mut wrong = original.clone();
        if at == 22 { wrong[at] = 1; } else { wrong[at] ^= 0x80; }
        assert!(Claim::decode(&wrong).is_ok());
        assert_eq!(expected.check_row(&wrong), Err(Invalid::DifferentRow));
    }
    let mut zero = original.clone();
    zero[71..].fill(0);
    assert!(Claim::decode(&zero).is_ok()); // digest is a fixed value, never a missing/skip sentinel
    assert_eq!(expected.check_row(&zero), Err(Invalid::DifferentRow));
    assert_eq!(expected.check_row(&vector("later_f")), Err(Invalid::DifferentRow));
    // Two identical copies have identical claims; this purely byte-level result is not physical evidence.
    assert_eq!(expected.check_row(&original.clone()), Ok(()));
}

#[test]
fn one_exact_control_slot_is_not_the_name_of_a_business_volume_or_an_entire_prefix() {
    assert_eq!(ROOT_SLOT.len(), 43);
    assert_eq!(&ROOT_SLOT[..9], b".zxs-new-");
    assert_eq!(&ROOT_SLOT[9..41], &[b'f'; 32]);
    assert_eq!(&ROOT_SLOT[41..], b"-0");
    assert_ne!(ROOT_SLOT, b"VOLUME");
}
