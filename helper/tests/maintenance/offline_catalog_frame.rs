use super::*;

const DESCRIPTOR: [u8; 32] = [0x41; 32];
const COUNT: std::ops::Range<usize> = 40..48;

fn framed(items: &[&[u8]]) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.extend_from_slice(MAGIC);
    frame.extend_from_slice(&DESCRIPTOR);
    frame.extend_from_slice(&u64::try_from(items.len()).unwrap().to_be_bytes());
    for item in items {
        frame.extend_from_slice(&u16::try_from(item.len()).unwrap().to_be_bytes());
        frame.extend_from_slice(item);
    }
    frame.extend_from_slice(END);
    frame
}

#[test]
fn catalog_frames_preserve_case_and_require_a_complete_footer_and_eof() {
    let good = framed(&[b"A", b"a", "方案".as_bytes()]);
    assert_eq!(
        decode_frame(&mut good.as_slice(), DESCRIPTOR).unwrap(),
        ["A", "a", "方案"]
    );
    assert_eq!(
        decode_frame(&mut framed(&[]).as_slice(), DESCRIPTOR).unwrap(),
        Vec::<String>::new()
    );
    for cut in 0..good.len() {
        assert!(decode_frame(&mut &good[..cut], DESCRIPTOR).is_err());
    }
    let mut trailing = good.clone();
    trailing.push(0);
    assert!(decode_frame(&mut trailing.as_slice(), DESCRIPTOR).is_err());
    let mut huge_count = good;
    huge_count[COUNT].copy_from_slice(&u64::MAX.to_be_bytes());
    assert!(decode_frame(&mut huge_count.as_slice(), DESCRIPTOR).is_err());
}

#[test]
fn a_well_formed_catalog_must_bind_to_the_exact_original_descriptor() {
    let good = framed(&[b"report"]);
    assert!(decode_frame(&mut good.as_slice(), [0x42; 32]).is_err());
    let mut wrong = good;
    wrong[8] ^= 1;
    assert!(decode_frame(&mut wrong.as_slice(), DESCRIPTOR).is_err());
}

#[test]
fn catalog_frame_rejects_duplicate_invalid_or_oversize_names() {
    let oversized = [b'x'; 256];
    for values in [
        vec![b"a".as_slice(), b"a".as_slice()],
        vec![b"a".as_slice(), b"A".as_slice()],
        vec![b"not/a/component".as_slice()],
        vec![b"\xff".as_slice()],
        vec![b"".as_slice()],
        vec![oversized.as_slice()],
    ] {
        assert!(decode_frame(&mut framed(&values).as_slice(), DESCRIPTOR).is_err());
    }
}
