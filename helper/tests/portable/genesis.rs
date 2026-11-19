use super::{Initial, Invalid, GENESIS, MAX_GENESIS};
use sha2::{Digest, Sha256};

const VECTORS: &str = include_str!("genesis-v1.txt");
fn vector(key: &str) -> Vec<u8> {
    let hex = VECTORS.lines().filter_map(|ln| ln.split_once('='))
        .find(|(name, _)| *name == key).expect("fixed vector").1;
    assert!(hex.is_ascii() && hex.len().is_multiple_of(2));
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i+2], 16).unwrap()).collect()
}

fn signed(body: &[u8]) -> Vec<u8> {
    let mut raw = body.to_vec();
    raw.extend_from_slice(&Sha256::digest(body));
    raw
}

#[test]
fn one_canonical_request_supplies_the_four_independent_markers_and_the_zero_floor() {
    let body = vector("body");
    let image = vector("genesis");
    assert_eq!((body.len(), image.len(), MAX_GENESIS), (183, 215, 263_533));
    assert_eq!(Sha256::digest(&body).as_slice(), vector("source_digest").as_slice());
    assert_eq!(signed(&body), image);
    let original = Initial::decode(&image).unwrap();
    assert_eq!(original.check_request(&body), Ok(()));
    let expected: [(&[u8], u8, &str); 4] = [
        (b"/m/sys", 255, "volume_s"), (b"/m/bin", 240, "volume_r"),
        (b"/m/data", 255, "volume_f0"), (b"/m/\xffvol", 200, "volume_f1"),
    ];
    for (i, (path, limit, key)) in expected.into_iter().enumerate() {
        let (actual_path, actual_limit, claim) = original.row(i).unwrap();
        assert_eq!((actual_path, actual_limit), (path, limit));
        assert_eq!(claim.encode().as_slice(), vector(key).as_slice());
    }
    assert!(matches!(original.row(4), Err(Invalid::Feedback)));
    let s = vector("volume_s");
    let floor = vector("zero_floor");
    assert_eq!(floor.len(), 105);
    assert_eq!(original.check_system_fields(b"/m/sys", 1001, &s, &floor), Ok(()));
    assert_eq!(original.check_system_fields(b"/m/data", 1001, &s, &floor), Err(Invalid::System));
    assert_eq!(original.check_system_fields(b"/m/sys", 1002, &s, &floor), Err(Invalid::System));
    assert_eq!(original.check_system_fields(b"/m/sys", 1001, &vector("volume_r"), &floor), Err(Invalid::Feedback));
}

#[test]
fn rehashing_does_not_legitimize_bad_paths_order_duplicate_rows_or_another_approval() {
    let b = vector("body");
    // Row starts in the independently fixed 183-byte body (20 metadata bytes per row).
    let (s, r, f0, f1) = (77, 103, 129, 156);
    for (at, replacement, why) in [
        (23, &[0;16][..], Invalid::Identity), (39, &[0;16][..], Invalid::Identity),
        (55, &[0;16][..], Invalid::Identity), (71, &[0;4][..], Invalid::Owner),
        (71, &[255;4][..], Invalid::Owner), (75, &[0,2][..], Invalid::Role),
        (75, &[0,65][..], Invalid::Role), (s, &[1][..], Invalid::Role),
        (r+1, &b[s+1..s+17], Invalid::Identity), (r+17, &[0][..], Invalid::ComponentLimit),
        (r+20, b"/m/sys".as_slice(), Invalid::Path), (f0+20, b"/m/../a".as_slice(), Invalid::Path),
        (f0+20, b"/m//abc".as_slice(), Invalid::Path), (f1+20, b"/m/bad\0".as_slice(), Invalid::Path),
        (f0+18, &[0x10,0x01][..], Invalid::Path),
    ] {
        let mut changed = b.clone();
        changed[at..at+replacement.len()].copy_from_slice(replacement);
        assert_eq!(Initial::decode(&signed(&changed)).err(), Some(why));
    }
    let mut nested = b.clone(); // F1 follows F0 lexically but is inside it
    nested.splice(f1+18.., [0,9].into_iter().chain(*b"/m/data/x"));
    assert_eq!(Initial::decode(&signed(&nested)).err(), Some(Invalid::Path));
    let mut unordered = b[..f0].to_vec(); unordered.extend_from_slice(&b[f1..]); unordered.extend_from_slice(&b[f0..f1]);
    assert_eq!(Initial::decode(&signed(&unordered)).err(), Some(Invalid::Path));

    for at in [23,39,55,74,r+1,r+17] {
        let mut changed = b.clone(); changed[at] ^= 1;
        let image = signed(&changed);
        let other = Initial::decode(&image).unwrap();
        assert_eq!(other.check_request(&b), Err(Invalid::Request));
        let marker = other.row(0).unwrap().2;
        assert!(marker.check_row(&vector("volume_s")).is_err());
    }
}

#[test]
fn a_valid_pending_prefix_is_not_permission_to_overlook_a_gap_or_a_missing_floor() {
    let image = vector("genesis");
    let g = Initial::decode(&image).unwrap();
    let raw = ["volume_s","volume_r","volume_f0","volume_f1"].map(vector);
    let full = raw.each_ref().map(|m| Some(m.as_slice()));
    let floor = vector("zero_floor");
    assert_eq!(g.pending_feedback(None, &[None; 4]), Ok(0));
    for n in 0..=4 {
        let mut p = full; p[n..].fill(None);
        assert_eq!(g.pending_feedback(Some(&floor), &p), Ok(n));
        if n > 0 { assert_eq!(g.pending_feedback(None, &p), Err(Invalid::Feedback)); }
    }
    let mut hole = full; hole[1] = None;
    assert_eq!(g.pending_feedback(Some(&floor), &hole), Err(Invalid::Feedback));
    assert_eq!(g.pending_feedback(Some(&floor), &full[..3]), Err(Invalid::Feedback));
    let mut wrong = full; wrong.swap(2,3);
    assert_eq!(g.pending_feedback(Some(&floor), &wrong), Err(Invalid::Feedback));
    assert_eq!(g.pending_feedback(Some(&[]), &[None;4]), Err(Invalid::Floor));
    // Copies still have the same logical bytes; no physical or phase proof was returned.
    let copy = raw[0].clone();
    assert_eq!(g.pending_feedback(Some(&floor), &[Some(copy.as_slice()),None,None,None]), Ok(1));
}

#[test]
fn an_unrecognized_suffix_or_claimed_nonzero_frontier_is_never_an_empty_registration() {
    let image = vector("genesis");
    for end in 0..image.len() { assert!(Initial::decode(&image[..end]).is_err()); }
    let mut old = image.clone(); old[..GENESIS.len()].fill(b'x');
    assert_eq!(Initial::decode(&old).err(), Some(Invalid::Format));
    let mut bad = image.clone(); *bad.last_mut().unwrap() ^= 1;
    assert_eq!(Initial::decode(&bad).err(), Some(Invalid::Digest));
    let mut unknown = image.clone(); unknown.extend_from_slice(&vector("unknown_tail"));
    assert_eq!(Initial::decode(&unknown).err(), Some(Invalid::TailNotCovered));
    let mut much_later = image.clone(); much_later.resize(MAX_GENESIS+1,0);
    assert_eq!(Initial::decode(&much_later).err(), Some(Invalid::TailNotCovered));

    let g = Initial::decode(&image).unwrap();
    let s = vector("volume_s");
    let floor = vector("zero_floor");
    for at in [0,24,25,56,57,64,65,72,73,104] {
        let mut wrong = floor.clone(); wrong[at] ^= 1;
        assert_eq!(g.check_system_fields(b"/m/sys",1001,&s,&wrong), Err(Invalid::Floor));
    }
    for rejected in [vec![], floor[..104].to_vec(), [floor.as_slice(), &[0]].concat(), vector("untrusted_nonzero_floor")] {
        assert_eq!(g.check_system_fields(b"/m/sys", 1001, &s, &rejected), Err(Invalid::Floor));
    }
}

#[test]
fn every_allowed_row_can_reach_the_byte_limit_without_an_extra_row_or_byte() {
    let mut body = vector("body")[..75].to_vec();
    body.extend_from_slice(&64u16.to_be_bytes());
    let mut last_len_at = 0;
    for i in 0u8..64 {
        body.push(if i < 2 { i } else { 2 });
        body.extend_from_slice(&[i + 1; 16]);
        body.push(255);
        last_len_at = body.len();
        body.extend_from_slice(&4096u16.to_be_bytes());
        // 4 bytes, then fifteen 256-byte components (slash included),
        // then a slash and 251 bytes. Distinct, sorted, no aliases/prefixes.
        body.extend_from_slice(format!("/v{i:02}").as_bytes());
        for _ in 0..15 { body.push(b'/'); body.extend_from_slice(&[b'x'; 255]); }
        body.push(b'/'); body.extend_from_slice(&[b'y'; 251]);
    }
    let image = signed(&body);
    assert_eq!(image.len(), MAX_GENESIS);
    let g = Initial::decode(&image).unwrap();
    assert_eq!(g.row(63).unwrap().0.len(), 4096);
    assert!(g.row(64).is_err());
    body[last_len_at..last_len_at+2].copy_from_slice(&4097u16.to_be_bytes());
    body.push(b'z');
    assert_eq!(Initial::decode(&signed(&body)).err(), Some(Invalid::Path));
}
