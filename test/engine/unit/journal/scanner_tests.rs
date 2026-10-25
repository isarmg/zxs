use super::*;
use std::{cell::Cell, io::Cursor};

fn frame(record: &Record) -> Vec<u8> {
    let payload = canonical_json(record).unwrap();
    let mut bytes = LOG_MAGIC.to_vec();
    bytes.extend((payload.len() as u32).to_le_bytes());
    bytes.extend(payload);
    bytes.extend(record_frame_hash(record).unwrap().unwrap().as_bytes());
    bytes
}

fn records() -> Vec<Record> {
    let mut previous = "0".repeat(64);
    (1..=24)
        .map(|sequence| {
            let record = Record {
                record_type: "PREPARE".into(),
                sequence,
                candidate_hash: format!("{sequence:064x}"),
                object_hash: "b".repeat(64),
                previous_frame_hash: previous.clone(),
            };
            previous = record_frame_hash(&record)
                .unwrap()
                .unwrap()
                .to_hex()
                .to_string();
            record
        })
        .collect()
}

struct ShortReader<'a> {
    bytes: Cursor<&'a [u8]>,
    maximum: usize,
    reads: &'a Cell<usize>,
}
impl Read for ShortReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.reads.set(self.reads.get() + 1);
        let length = buffer.len().min(self.maximum);
        self.bytes.read(&mut buffer[..length])
    }
}

#[test]
fn buffered_scan_preserves_bytes_and_hashes_across_short_reads() {
    let expected = records();
    let bytes: Vec<_> = expected.iter().flat_map(frame).collect();
    for maximum in [1, 3, 11, 4096] {
        for capacity in [1, 7, 8192] {
            let reads = Cell::new(0);
            let reader = ShortReader {
                bytes: Cursor::new(bytes.as_slice()),
                maximum,
                reads: &reads,
            };
            let scanned = scan_frames(BufReader::with_capacity(capacity, reader)).unwrap();
            assert_eq!(scanned.valid_bytes, bytes.len() as u64);
            assert!(!scanned.torn_tail);
            assert!(
                scanned.stamp.is_none(),
                "only the file wrapper binds identity"
            );
            assert_eq!(scanned.records.len(), expected.len());
            for ((record, hash), expected) in scanned.records.iter().zip(&expected) {
                assert_eq!(
                    canonical_json(record).unwrap(),
                    canonical_json(expected).unwrap()
                );
                assert_eq!(
                    hash,
                    record_frame_hash(expected)
                        .unwrap()
                        .unwrap()
                        .to_hex()
                        .as_str()
                );
            }
        }
    }
}

#[test]
fn buffering_coalesces_frame_reads_without_changing_results() {
    let bytes: Vec<_> = records().iter().flat_map(frame).collect();
    let unbuffered_reads = Cell::new(0);
    let unbuffered = scan_frames(ShortReader {
        bytes: Cursor::new(bytes.as_slice()),
        maximum: usize::MAX,
        reads: &unbuffered_reads,
    })
    .unwrap();
    let buffered_reads = Cell::new(0);
    let buffered = scan_frames(BufReader::new(ShortReader {
        bytes: Cursor::new(bytes.as_slice()),
        maximum: usize::MAX,
        reads: &buffered_reads,
    }))
    .unwrap();
    assert_eq!(buffered.valid_bytes, unbuffered.valid_bytes);
    assert_eq!(buffered.records.len(), unbuffered.records.len());
    assert!(buffered_reads.get() < unbuffered_reads.get());
}

#[test]
fn buffered_scan_keeps_every_partial_frame_as_a_torn_tail() {
    let records = records();
    let prefix = frame(&records[0]);
    let tail = frame(&records[1]);
    for cut in 0..tail.len() {
        let mut bytes = prefix.clone();
        bytes.extend(&tail[..cut]);
        let scanned = scan_frames(BufReader::with_capacity(7, bytes.as_slice())).unwrap();
        assert_eq!(scanned.valid_bytes, prefix.len() as u64);
        assert_eq!(scanned.records.len(), 1);
        assert_eq!(scanned.torn_tail, cut != 0);
    }
    let empty = scan_frames(BufReader::new(&[][..])).unwrap();
    assert_eq!(empty.valid_bytes, 0);
    assert!(!empty.torn_tail);
}

#[test]
fn buffered_scan_does_not_skip_corrupt_or_unsupported_frames() {
    let records = records();
    for change in 0..6 {
        let mut record = records[1].clone();
        match change {
            3 => record.previous_frame_hash = "f".repeat(64),
            4 => record.sequence = 0,
            5 => record.record_type = "UNSUPPORTED".into(),
            _ => {}
        }
        let mut tail = frame(&record);
        match change {
            0 => tail[0] ^= 1,
            1 => tail[8..12].copy_from_slice(&((MAX_FRAME + 1) as u32).to_le_bytes()),
            2 => *tail.last_mut().unwrap() ^= 1,
            _ => {}
        }
        let bytes = [frame(&records[0]), tail, frame(&records[2])].concat();
        let error = scan_frames(BufReader::with_capacity(7, bytes.as_slice()))
            .err()
            .unwrap();
        assert_eq!(
            error.code(),
            if change == 5 {
                "LOG_FORMAT_UNSUPPORTED"
            } else {
                "LOG_CORRUPT"
            }
        );
    }
}

#[test]
fn buffered_scan_propagates_read_errors_instead_of_accepting_a_partial_tail() {
    struct FailingReader;
    impl Read for FailingReader {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        }
    }
    assert!(scan_frames(BufReader::new(FailingReader)).is_err());
}
