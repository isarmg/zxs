use super::*;
use std::os::unix::fs::FileExt;

#[test]
fn adjacent_working_blocks_authenticate_one_real_base_chunk_then_verify_the_next() {
    const BLOCK: usize = WORKING_BLOCK_BYTES as usize;
    let body: Vec<_> = (0..4 * BLOCK).map(|index| (index / BLOCK) as u8 + 5).collect();
    let file = tempfile::tempfile().unwrap();
    file.write_all_at(&body, 0).unwrap();
    let chunks = body.chunks(2 * BLOCK).map(|bytes| *blake3::hash(bytes).as_bytes()).collect();
    let checksums = ChecksumManifest {
        chunk_bytes: (2 * BLOCK) as u32,
        logical_size: body.len() as u64,
        chunks,
    };
    let mut range = BaseRange::new(&checksums);
    let mut reads = Vec::new();
    let mut read = |bytes: &mut [u8], offset: u64| {
        reads.push((offset, bytes.len()));
        file.read_exact_at(bytes, offset)
    };
    let mut buffer = [0; BLOCK];
    range.block(0, u64::MAX, &mut buffer, &mut read).unwrap();
    assert_eq!(&buffer[..], &body[..BLOCK]);
    range.block(1, u64::MAX, &mut buffer, &mut read).unwrap();
    assert_eq!(&buffer[..], &body[BLOCK..2 * BLOCK]);
    // Only the retained part is visible; all bytes in its original physical
    // checksum chunk are nevertheless authenticated.
    range.block(2, (2 * BLOCK + 31) as u64, &mut buffer, &mut read).unwrap();
    assert_eq!(&buffer[..31], &body[2 * BLOCK..2 * BLOCK + 31]);
    assert!(buffer[31..].iter().all(|byte| *byte == 0));
    range.block(3, (2 * BLOCK + 31) as u64, &mut buffer, &mut read).unwrap();
    assert!(buffer.iter().all(|byte| *byte == 0));
    assert_eq!(reads, [(0, 2 * BLOCK), ((2 * BLOCK) as u64, 2 * BLOCK)]);

    // A later request never reuses the first request's old verified bytes.
    file.write_all_at(&[255], 17).unwrap();
    let mut another = BaseRange::new(&checksums);
    assert!(another.block(1, u64::MAX, &mut buffer, |bytes, at| {
        file.read_exact_at(bytes, at)
    }).is_err());
}

#[test]
fn a_missing_checksum_never_exposes_an_unverified_requested_block() {
    let checksums = ChecksumManifest {
        chunk_bytes: WORKING_BLOCK_BYTES as u32,
        logical_size: WORKING_BLOCK_BYTES,
        chunks: Vec::new(),
    };
    let mut range = BaseRange::new(&checksums);
    let mut bytes = [0; WORKING_BLOCK_BYTES as usize];
    assert!(range.block(0, u64::MAX, &mut bytes, |_, _| {
        panic!("missing physical checksum must fail before read")
    }).is_err());
}
