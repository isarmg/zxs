//! Real snapshot decoding/admission, not a model of the segmented log reader.
use super::*;
use storage_contract::{
    EVENT_SEGMENT_KIND, EVENT_SEGMENT_SPAN, EventDirectoryEntry, EventSegment,
    OPERATION_SEGMENT_KIND, OPERATION_SEGMENT_RECORDS, OPERATION_SEGMENT_TARGET_BYTES,
    OperationDirectoryEntry, OperationRecordEntry, OperationSegment,
};

#[path = "segments_support.rs"]
mod support;
use support::*;

#[test]
fn valid_sparse_and_equal_sequences_survive_complete_snapshot_admission() {
    let mut graph = Graph::default();
    graph.events(0, &vec![3; EVENT_SEGMENT_SPAN]);
    graph.events(1, &[3, 19]);
    graph.receipts(0, 0, full_receipts(0, 3));
    graph.receipts(1, OPERATION_SEGMENT_RECORDS as u64, vec![receipt(999, 19)]);
    let state = graph.resolve(19).unwrap();
    assert_eq!(state.events.len(), EVENT_SEGMENT_SPAN as u64 + 2);
    assert_eq!(state.events.tail(), &[event(3), event(19)]);
    assert_eq!(state.operations.tail(), &[receipt(999, 19)]);
    assert_eq!(state.operations.sealed().len(), 1);
    assert_eq!(state.operations.sealed()[0].records, OPERATION_SEGMENT_RECORDS as u64);
}

#[test]
fn event_span_directory_and_cross_segment_mutations_are_rejected() {
    for sequences in [vec![], vec![1; EVENT_SEGMENT_SPAN + 1]] {
        let mut graph = Graph::default();
        graph.events(0, &sequences);
        expect_error(&graph, 100, ErrorCode::ObjectCorrupt, "event segment span differs from the seal rule");
    }
    let mut short_prefix = Graph::default();
    short_prefix.events(0, &[1]);
    short_prefix.events(1, &[2]);
    expect_error(&short_prefix, 2, ErrorCode::ObjectCorrupt, "event segment span differs from the seal rule");

    let mut gap = Graph::default();
    gap.events(1, &[1]);
    expect_error(&gap, 1, ErrorCode::ObjectCorrupt, "segment directory ordinals are not contiguous");

    let mut first = Graph::default();
    first.events(0, &[2]);
    first.events.get_mut(&format!("{:020}", 0)).unwrap()["first_sequence"] = "1".into();
    expect_error(&first, 2, ErrorCode::ObjectCorrupt, "event segment sequences are out of order");
    let mut internal = Graph::default();
    internal.events(0, &[2, 1]);
    expect_error(&internal, 2, ErrorCode::ObjectCorrupt, "event segment sequences are out of order");
    let mut cross = Graph::default();
    cross.events(0, &vec![2; EVENT_SEGMENT_SPAN]);
    cross.events(1, &[1]);
    expect_error(&cross, 2, ErrorCode::ObjectCorrupt, "event segment sequences are out of order");
    let mut future = Graph::default();
    future.events(0, &[2]);
    expect_error(&future, 1, ErrorCode::StateInvalid, "snapshot event sequence exceeds published sequence");
}

#[test]
fn operation_directory_count_ordinal_and_sequence_are_checked_against_bodies() {
    for (field, value) in [("records", "0"), ("records", "18446744073709551615"), ("first_ordinal", "1"), ("first_sequence", "2")] {
        let mut graph = Graph::default();
        graph.receipts(0, 0, vec![receipt(1, 1)]);
        graph.operations.get_mut(&format!("{:020}", 0)).unwrap()[field] = value.into();
        expect_error(&graph, 2, ErrorCode::ObjectCorrupt, "operation segment differs from its directory entry");
    }
    let mut empty = Graph::default();
    empty.receipts(0, 0, vec![]);
    expect_error(&empty, 1, ErrorCode::ObjectCorrupt, "operation segment differs from its directory entry");
    let mut gap = Graph::default();
    gap.receipts(1, 0, vec![receipt(1, 1)]);
    expect_error(&gap, 1, ErrorCode::ObjectCorrupt, "segment directory ordinals are not contiguous");
    let mut internal = Graph::default();
    internal.receipts(0, 0, vec![receipt(1, 2), receipt(2, 1)]);
    expect_error(&internal, 2, ErrorCode::ObjectCorrupt, "operation segment sequences are out of order");
    let mut cross = Graph::default();
    cross.receipts(0, 0, full_receipts(0, 2));
    cross.receipts(1, OPERATION_SEGMENT_RECORDS as u64, vec![receipt(999, 1)]);
    expect_error(&cross, 2, ErrorCode::ObjectCorrupt, "operation segment sequences are out of order");
    let mut future = Graph::default();
    future.receipts(0, 0, vec![receipt(1, 2)]);
    expect_error(&future, 1, ErrorCode::StateInvalid, "snapshot operation sequence exceeds published sequence");
}

#[test]
fn operation_first_seal_rule_is_enforced_at_count_and_exact_byte_thresholds() {
    let mut count = Graph::default();
    let mut extra = full_receipts(0, 1);
    extra.push(receipt(999, 1));
    count.receipts(0, 0, extra);
    expect_error(&count, 1, ErrorCode::ObjectCorrupt, "operation segment continues beyond the seal rule");
    let mut short_prefix = Graph::default();
    short_prefix.receipts(0, 0, vec![receipt(0, 1)]);
    short_prefix.receipts(1, 1, vec![receipt(1, 1)]);
    expect_error(&short_prefix, 1, ErrorCode::ObjectCorrupt, "operation segment ends before the seal rule");

    // Padding is ASCII in a JSON string; one byte changes the canonical record
    // size by exactly one. The wrapper/array bytes do not count toward sealing.
    let mut boundary = receipt(0, 1);
    boundary.result.result = json_codec::json!({"padding": ""});
    let overhead = canonical_json(&boundary).unwrap().len();
    let pad = OPERATION_SEGMENT_TARGET_BYTES - overhead;
    for (delta, sealed) in [(-1isize, false), (0, true), (1, true)] {
        let mut record = boundary.clone();
        record.result.result["padding"] = "x".repeat(pad.checked_add_signed(delta).unwrap()).into();
        assert_eq!(canonical_json(&record).unwrap().len(), OPERATION_SEGMENT_TARGET_BYTES.checked_add_signed(delta).unwrap());
        let mut graph = Graph::default();
        graph.receipts(0, 0, vec![record]);
        let state = graph.resolve(1).unwrap();
        assert_eq!(!state.operations.sealed().is_empty(), sealed);
        graph.receipts(1, 1, vec![receipt(1, 1)]);
        if sealed {
            assert_eq!(graph.resolve(1).unwrap().operations.tail(), &[receipt(1, 1)]);
        } else {
            expect_error(&graph, 1, ErrorCode::ObjectCorrupt, "operation segment ends before the seal rule");
        }
    }
}

#[test]
fn duplicate_operation_ids_and_locator_misbindings_are_not_accepted_as_absence() {
    let mut duplicate = Graph::default();
    duplicate.receipts(0, 0, full_receipts(0, 1));
    duplicate.receipts(1, OPERATION_SEGMENT_RECORDS as u64, vec![receipt(1, 2)]);
    expect_error(&duplicate, 2, ErrorCode::ObjectCorrupt, "operation id appears in two receipts");
    let mut base = Graph::default();
    base.receipts(0, 0, vec![receipt(1, 9), receipt(2, 9)]);
    let id = receipt(1, 9).result.operation_id.to_string();
    for ordinal in ["1", "9", "18446744073709551615"] {
        let mut wrong = base.clone();
        wrong.locators.insert(id.clone(), ordinal.into());
        expect_error(&wrong, 9, ErrorCode::ObjectCorrupt, "operation locators differ from the receipt log");
    }
    let mut absent = base.clone();
    absent.locators.remove(&id);
    expect_error(&absent, 9, ErrorCode::ObjectCorrupt, "operation locators differ from the receipt log");
    let mut extra = base.clone();
    extra.locators.insert(receipt(99, 9).result.operation_id.to_string(), "2".into());
    expect_error(&extra, 9, ErrorCode::ObjectCorrupt, "operation locators differ from the receipt log");
    for text in ["", "00", "-1", "+1", " 0", "18446744073709551616"] {
        let mut bad = base.clone();
        bad.locators.insert(id.clone(), text.into());
        let error = bad.resolve(9).unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidInteger, "{text:?}");
        if text == "18446744073709551616" {
            assert_eq!(error.message(), "unsigned decimal exceeds u64");
            assert!(std::error::Error::source(&error).is_some());
        } else {
            assert_eq!(error.message(), "expected a canonical unsigned decimal string");
        }
    }
}

#[test]
fn correctly_rehashed_segment_schema_mutations_preserve_json_error_source() {
    for kind in [EVENT_SEGMENT_KIND, OPERATION_SEGMENT_KIND] {
        for mutation in ["format", "unknown", "missing", "non_string_sequence"] {
            let mut graph = Graph::default();
            if kind == EVENT_SEGMENT_KIND { graph.events(0, &[1]); }
            else { graph.receipts(0, 0, vec![receipt(1, 1)]); }
            let directory = if kind == EVENT_SEGMENT_KIND { &mut graph.events } else { &mut graph.operations };
            let entry = directory.get_mut(&format!("{:020}", 0)).unwrap();
            let address = format!("{kind}/{}", entry["hash"].as_str().unwrap());
            let mut body = graph.objects.remove(&address).unwrap();
            match mutation {
                "format" => body["format"] = "unsupported-segment".into(),
                "unknown" => body["unexpected"] = true.into(),
                "missing" => { body.as_object_mut().unwrap().remove("format"); }
                "non_string_sequence" => {
                    if kind == EVENT_SEGMENT_KIND { body["events"][0]["sequence"] = 1.into(); }
                    else { body["records"][0]["result"]["sequence"] = 1.into(); }
                }
                _ => unreachable!(),
            }
            let hash = object_hash(kind, &body).unwrap();
            entry["hash"] = hash.clone().into();
            graph.objects.insert(format!("{kind}/{hash}"), body);
            let error = graph.resolve(1).unwrap_err();
            assert_eq!(error.code(), ErrorCode::InvalidJson, "{kind}/{mutation}");
            match &error {
                Error::Contract(storage_contract::Error::Json { source, .. }) => {
                    assert!(source.is_data());
                    assert!(std::error::Error::source(&error).is_some());
                }
                other => panic!("{kind}/{mutation}: expected JSON source, got {other:?}"),
            }
        }
    }
}

#[test]
fn fixed_seed_combinations_replay_at_published_sequence_and_locator_boundaries() {
    // The seed and finite case count are part of the committed corpus contract.
    // Each case starts with a known-valid two-receipt graph; independent
    // mutations have explicit precedence (future sequence before locators).
    let mut seed = 0x5a58_5320_2610_2901u64;
    for case in 0..64 {
        seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17;
        let sequence = 7 + seed % 1000;
        let mut graph = Graph::default();
        graph.events(0, &[sequence, sequence]);
        graph.receipts(0, 0, vec![receipt(1, sequence), receipt(2, sequence)]);
        let future = seed & 1 != 0;
        let mismatch = seed & 2 != 0;
        if mismatch { graph.locators.insert(receipt(1, sequence).result.operation_id.to_string(), "1".into()); }
        eprintln!("metadata-adversarial seed=0x5a58532026102901 case={case} state={seed:016x} future={future} locator={mismatch}");
        if future {
            expect_error(&graph, sequence - 1, ErrorCode::StateInvalid, "snapshot event sequence exceeds published sequence");
        } else if mismatch {
            expect_error(&graph, sequence, ErrorCode::ObjectCorrupt, "operation locators differ from the receipt log");
        } else {
            let state = graph.resolve(sequence).unwrap();
            assert_eq!(state.operations.tail().len(), 2);
            assert_eq!(state.events.tail(), &[event(sequence), event(sequence)]);
        }
    }
}

#[test]
fn disk_snapshot_rejects_segment_damage_without_rewriting_authoritative_objects() {
    for kind in [EVENT_SEGMENT_KIND, OPERATION_SEGMENT_KIND] {
        for mutation in ["digest", "truncated", "missing"] {
            let temp = tempfile::tempdir().unwrap();
            let config = Config::development([temp.path().join("member")]);
            let disk = &config.disks[0];
            let mut graph = Graph::default();
            graph.events(0, &[1]);
            graph.receipts(0, 0, vec![receipt(1, 1)]);
            let (snapshot, objects) = graph.snapshot(1);
            let mut authoritative = BTreeMap::new();
            for (address, value) in objects {
                let (object_kind, expected_hash) = address.split_once('/').unwrap();
                let hash = store_object(disk, object_kind, &value).unwrap();
                assert_eq!(hash, expected_hash);
                let path = object_path(disk, object_kind, &hash);
                authoritative.insert(path.clone(), std::fs::read(path).unwrap());
            }
            let snapshot_hash = store_object(disk, "snapshot", &snapshot).unwrap();
            let snapshot_path = object_path(disk, "snapshot", &snapshot_hash);
            authoritative.insert(snapshot_path.clone(), std::fs::read(snapshot_path).unwrap());
            assert_eq!(load_snapshot(disk, &snapshot_hash).unwrap().operations.tail().len(), 1);

            let directory = if kind == EVENT_SEGMENT_KIND { &graph.events } else { &graph.operations };
            let hash = directory[&format!("{:020}", 0)]["hash"].as_str().unwrap();
            let target = object_path(disk, kind, hash);
            match mutation {
                "digest" => {
                    let mut body: json_codec::Value = json_codec::from_slice(&authoritative[&target]).unwrap();
                    if kind == EVENT_SEGMENT_KIND { body["events"][0]["object_id"] = "damaged".into(); }
                    else { body["records"][0]["principal"] = "damaged".into(); }
                    let bytes = canonical_json(&body).unwrap();
                    std::fs::write(&target, &bytes).unwrap();
                    authoritative.insert(target.clone(), bytes);
                }
                "truncated" => {
                    std::fs::write(&target, b"{").unwrap();
                    authoritative.insert(target.clone(), b"{".to_vec());
                }
                "missing" => {
                    std::fs::remove_file(&target).unwrap();
                    authoritative.remove(&target);
                }
                _ => unreachable!(),
            }
            let error = load_snapshot(disk, &snapshot_hash).unwrap_err();
            match mutation {
                "digest" => {
                    assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
                    assert_eq!(error.message(), "immutable object digest differs");
                }
                "missing" => {
                    assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
                    assert_eq!(error.message(), "immutable object is missing");
                    assert_eq!(std::fs::symlink_metadata(&target).unwrap_err().kind(), std::io::ErrorKind::NotFound);
                }
                "truncated" => match &error {
                    Error::Contract(storage_contract::Error::Json { source, .. }) => {
                        assert_eq!(error.code(), ErrorCode::InvalidJson);
                        assert!(source.is_eof());
                    }
                    other => panic!("expected preserved truncated JSON source, got {other:?}"),
                },
                _ => unreachable!(),
            }
            // These are the authoritative immutable files, including the bad
            // one when present. No derived lock/index file is asserted here.
            for (path, bytes) in authoritative {
                assert_eq!(std::fs::read(path).unwrap(), bytes, "{kind}/{mutation}");
            }
        }
    }
}
