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
                record_type: RecordType::Prepare,
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
    for change in 0..5 {
        let mut record = records[1].clone();
        match change {
            3 => record.previous_frame_hash = "f".repeat(64),
            4 => record.sequence = 0,
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
        assert_eq!(error.code(), storage_contract::ErrorCode::LogCorrupt);
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
    let error = scan_frames(BufReader::new(FailingReader)).err().unwrap();
    assert_eq!(error.code(), ErrorCode::IoError);
    use std::error::Error as _;
    let mut source = error.source();
    let mut found = false;
    while let Some(cause) = source {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            assert_eq!(io.kind(), std::io::ErrorKind::PermissionDenied);
            found = true;
        }
        source = cause.source();
    }
    assert!(found, "journal read failure retains its I/O cause");
}

fn raw_frame(payload: &[u8]) -> Vec<u8> {
    let mut bytes = b"ZXSLOG\0\0".to_vec();
    bytes.extend((payload.len() as u32).to_le_bytes());
    bytes.extend(payload);
    let hash = blake3::hash(&bytes);
    bytes.extend(hash.as_bytes());
    bytes
}

#[test]
fn phase_strings_preserve_canonical_payload_frame_and_hash() {
    for (phase, token, payload_len, expected_hash) in [
        (
            RecordType::Prepare,
            "PREPARE",
            294,
            "358308f300921f2b592a2c9e512ada59c9cf3e740730bc3dc4207c8a1ace18a7",
        ),
        (
            RecordType::Accept,
            "ACCEPT",
            293,
            "181fd9ba33af53d3dcac8386b75eb0aed44c1ef74b4cfaccc545ada1a8d72086",
        ),
        (
            RecordType::Finalize,
            "FINALIZE",
            295,
            "ffd91c5c9127a579d7bd1ea3d0fd9ac28b1de17f0a1ed11977186af247b8f8fc",
        ),
        (
            RecordType::Publication,
            "PUBLICATION",
            298,
            "98f671d06922969d3d56bcdca9bc33df44a1c4f02c38e2648af6a7fb931b987e",
        ),
        (
            RecordType::Reprotect,
            "REPROTECT",
            296,
            "b1b77c8c60ebec2332a7b2359f228b3824f3d5603991c2d52c6321f032ea7924",
        ),
    ] {
        let record = Record {
            record_type: phase,
            sequence: 1,
            candidate_hash: "a".repeat(64),
            object_hash: "b".repeat(64),
            previous_frame_hash: "0".repeat(64),
        };
        let payload = format!(
            r#"{{"candidate_hash":"{}","object_hash":"{}","previous_frame_hash":"{}","record_type":"{token}","sequence":"1"}}"#,
            "a".repeat(64),
            "b".repeat(64),
            "0".repeat(64)
        );
        assert_eq!(payload.len(), payload_len);
        assert_eq!(canonical_json(&record).unwrap(), payload.as_bytes());
        assert_eq!(json_codec::to_value(&record).unwrap()["record_type"], token);
        let expected = raw_frame(payload.as_bytes());
        assert_eq!(frame(&record), expected);
        assert_eq!(
            record_frame_hash(&record)
                .unwrap()
                .unwrap()
                .to_hex()
                .as_str(),
            expected_hash
        );
        let scanned = scan_frames(expected.as_slice()).unwrap();
        assert_eq!(scanned.records[0].0.record_type, phase);
        assert_eq!(scanned.records[0].1, expected_hash);
    }
}

#[test]
fn raw_phase_decoder_rejects_unknown_and_non_string_shapes() {
    use std::error::Error as _;
    for phase in [
        json_codec::json!("UNSUPPORTED"),
        json_codec::json!("prepare"),
        json_codec::json!(""),
        json_codec::json!(null),
        json_codec::json!(1),
        json_codec::json!(true),
        json_codec::json!([]),
        json_codec::json!({"PREPARE": null}),
        json_codec::json!({"ACCEPT": null}),
        json_codec::json!({"FINALIZE": null}),
        json_codec::json!({"PUBLICATION": null}),
        json_codec::json!({"REPROTECT": null}),
    ] {
        let mut raw = json_codec::to_value(&records()[0]).unwrap();
        raw["record_type"] = phase.clone();
        let payload = canonical_json(&raw).unwrap();
        assert_eq!(
            decode_strict::<Record>(&payload).unwrap_err().code(),
            storage_contract::ErrorCode::InvalidJson,
            "{phase}"
        );
        let receipt = json_codec::json!({
            "disk_id": "disk", "slot_id": "P01", "frame_hash": "0".repeat(64),
            "record": raw,
        });
        assert!(
            decode_strict::<Receipt>(&canonical_json(&receipt).unwrap()).is_err(),
            "{phase}"
        );
        let bytes = raw_frame(&payload);
        let error = scan_frames(bytes.as_slice()).err().unwrap();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::LogCorrupt,
            "{phase}"
        );
        let mut source = error.source();
        let mut found = false;
        while let Some(cause) = source {
            found |= cause.is::<json_codec::Error>();
            source = cause.source();
        }
        assert!(
            found,
            "malformed stored phase retains its JSON cause: {phase}"
        );
    }
}

#[test]
fn typed_phases_survive_normal_publication_and_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let mut engine = Engine::initialize(
        &root,
        Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))),
    )
    .unwrap();
    let operation = storage_contract::OperationId::generate();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("typed phases").unwrap(),
            "P01",
            1,
            "alice",
            operation,
        )
        .unwrap();
    let result = engine.operation(operation).unwrap();
    for disk in engine.members() {
        let phases = scan(&disk)
            .unwrap()
            .records
            .into_iter()
            .filter(|(r, _)| r.sequence == result.sequence)
            .map(|(r, _)| r.record_type)
            .collect::<Vec<_>>();
        assert_eq!(
            phases,
            [
                RecordType::Prepare,
                RecordType::Accept,
                RecordType::Finalize,
                RecordType::Publication
            ]
        );
    }
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    let recovered = engine.operation(operation).unwrap();
    assert_eq!(recovered.sequence, result.sequence);
    assert_eq!(
        recovered.commit_state,
        storage_contract::CommitState::Published
    );
}

#[test]
fn valid_checksum_does_not_admit_noncanonical_receipt_payload() {
    let canonical = canonical_json(&records()[0]).unwrap();
    let mut noncanonical = b" \n".to_vec();
    noncanonical.extend(canonical);
    assert!(decode_strict::<Record>(&noncanonical).is_ok());
    let frame = raw_frame(&noncanonical);
    assert_eq!(
        scan_frames(frame.as_slice()).err().unwrap().code(),
        ErrorCode::LogCorrupt
    );
}

#[test]
fn operation_id_candidate_preserves_current_canonical_bytes_and_digest() {
    let bytes = br#"{"history_roots":{"hash":"ea047f909a3791b1133c3a2dd3c41e0c454897720aa8101e5f951b69a060f0ab","kind":"state-page"},"metadata_disk_ids":["disk"],"operation_id":"op_0123456789abcdef0123456789abcdef","pool_id":"pool","previous_snapshot":null,"request_digest":"request","sequence":"1","snapshot_hash":"snapshot","transaction_id":"transaction","versions":[]}"#;
    let candidate: Candidate = json_codec::from_slice(bytes).unwrap();
    assert_eq!(canonical_json(&candidate).unwrap(), bytes);
    let mut expected = blake3::Hasher::new();
    expected.update(b"zxs:candidate\0");
    expected.update(bytes);
    assert_eq!(
        object_hash("candidate", &candidate).unwrap(),
        expected.finalize().to_hex().as_str()
    );
}
