use super::*;

fn fixture() -> (tempfile::TempDir, Engine) {
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([temp.path().join("disk")]),
    )
    .unwrap();
    (temp, engine)
}

fn publish(engine: &mut Engine, id: OperationId, padding: usize) -> Result<OperationResult> {
    engine.mutate(
        "fixture.receipt",
        "owner",
        id,
        json_codec::json!({"padding": padding}),
        |_, _, _| Ok(json_codec::json!({"padding": "x".repeat(padding)})),
    )
}

#[test]
fn count_seal_publication_and_cold_replay_keep_every_permanent_receipt() {
    let (temp, mut engine) = fixture();
    let mut published = Vec::new();
    for _ in 0..storage_contract::OPERATION_SEGMENT_RECORDS {
        published.push(publish(&mut engine, OperationId::generate(), 0).unwrap());
    }
    assert_eq!(engine.state.operations.sealed().len(), 1);
    assert!(engine.state.operations.tail().is_empty());
    let fresh = engine.state.operations.fresh_seal().unwrap().1;
    let first = &fresh.records[0];
    let lookup = engine.operation_lookup(first.result.operation_id).unwrap();
    let Cow::Borrowed(entry) = &lookup.entry else {
        panic!("freshly sealed receipt was cloned before ownership authorization");
    };
    assert!(std::ptr::eq(*entry, first));
    assert_eq!(lookup.into_record().unwrap().into_result(), first.result);

    // Starting the next transaction releases the preceding fresh seal;
    // its new receipt remains in the tail.
    published.push(publish(&mut engine, OperationId::generate(), 0).unwrap());
    assert_eq!(engine.state.operations.sealed().len(), 1);
    assert_eq!(engine.state.operations.tail().len(), 1);
    assert!(engine.state.operations.fresh_seal().is_none());
    let tail = &engine.state.operations.tail()[0];
    let lookup = engine.operation_lookup(tail.result.operation_id).unwrap();
    let Cow::Borrowed(entry) = &lookup.entry else {
        panic!("tail receipt was cloned before ownership authorization");
    };
    assert!(std::ptr::eq(*entry, tail));
    assert_eq!(lookup.into_record().unwrap().into_result(), tail.result);
    let lookup = engine.operation_lookup(published[0].operation_id).unwrap();
    assert!(matches!(&lookup.entry, Cow::Owned(_)));
    assert_eq!(lookup.owner(), "owner");
    assert_eq!(lookup.into_record().unwrap().into_result(), published[0]);
    for result in &published {
        assert_eq!(
            engine
                .published_receipt(result.operation_id)
                .unwrap()
                .as_ref(),
            Some(result)
        );
    }
    drop(engine);
    let mut reopened = Engine::open(&temp.path().join("system")).unwrap();
    let lookup = reopened
        .operation_lookup(published[0].operation_id)
        .unwrap();
    assert!(matches!(&lookup.entry, Cow::Owned(_)));
    assert_eq!(lookup.owner(), "owner");
    assert_eq!(lookup.into_record().unwrap().into_result(), published[0]);
    for result in &published {
        assert_eq!(
            publish(&mut reopened, result.operation_id, 0).unwrap(),
            *result
        );
    }
    assert_eq!(reopened.state.operations.len(), published.len() as u64);
}

#[test]
fn byte_seal_pending_candidate_completes_after_restart() {
    let (temp, mut engine) = fixture();
    let id = OperationId::generate();
    let padding = storage_contract::OPERATION_SEGMENT_TARGET_BYTES;
    engine.inject_fault_once("after_finalize");
    assert_eq!(
        publish(&mut engine, id, padding).unwrap_err().code(),
        ErrorCode::InDoubt
    );
    drop(engine);
    let mut reopened = Engine::open(&temp.path().join("system")).unwrap();
    assert!(!reopened.is_read_only());
    assert_eq!(reopened.state.operations.sealed().len(), 1);
    assert!(reopened.state.operations.tail().is_empty());
    let result = reopened.published_receipt(id).unwrap().unwrap();
    assert_eq!(publish(&mut reopened, id, padding).unwrap(), result);
    publish(&mut reopened, OperationId::generate(), 0).unwrap();
    drop(reopened);
    let reopened = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(reopened.published_receipt(id).unwrap(), Some(result));
}

#[test]
fn pending_receipt_rejects_overflowing_or_reversed_directory_ranges() {
    let (_temp, mut engine) = fixture();
    let result = publish(&mut engine, OperationId::generate(), 0).unwrap();
    let members = engine.members();
    let segment = storage_contract::OperationSegment {
        format: Default::default(),
        records: vec![storage_contract::OperationRecordEntry {
            principal: "owner".into(),
            result: result.clone(),
        }],
    };
    let hash = crate::journal::PreparedMetadataObject::new(
        storage_contract::OPERATION_SEGMENT_KIND,
        &segment,
        crate::disk::MAX_METADATA,
    )
    .unwrap()
    .store(&members)
    .unwrap();
    for (first_ordinal, records) in [(u64::MAX, 2), (1, 1)] {
        let mut snapshot =
            crate::journal::prepare_snapshot(&State::default(), crate::disk::MAX_METADATA)
                .unwrap()
                .descriptor;
        snapshot.maps.insert(
            "operation_locators".into(),
            crate::journal::stored_map(
                &members,
                [(result.operation_id.to_string(), json_codec::json!("0"))]
                    .into_iter()
                    .collect(),
            ),
        );
        snapshot.maps.insert(
            "operation_log".into(),
            crate::journal::stored_map(
                &members,
                [(
                    format!("{:020}", 0),
                    json_codec::to_value(storage_contract::OperationDirectoryEntry {
                        kind: Default::default(),
                        hash: hash.clone(),
                        first_sequence: result.sequence,
                        first_ordinal,
                        records,
                    })
                    .unwrap(),
                )]
                .into_iter()
                .collect(),
            ),
        );
        assert_eq!(
            snapshot_receipt(&engine, &members, &snapshot, result.operation_id, false)
                .unwrap_err()
                .code(),
            ErrorCode::ObjectCorrupt
        );
    }
}

#[test]
fn recovery_overlay_lookup_requires_the_last_receipt_in_the_last_segment() {
    let (_temp, mut engine) = fixture();
    let first = publish(&mut engine, OperationId::generate(), 0).unwrap();
    let last = publish(&mut engine, OperationId::generate(), 0).unwrap();
    let members = engine.members();
    let snapshot: Snapshot = verified_object_at(
        &engine.disk_resources,
        &members,
        "snapshot",
        engine.snapshot.as_deref().unwrap(),
    )
    .unwrap();
    assert_eq!(
        snapshot_receipt(&engine, &members, &snapshot, first.operation_id, true)
            .unwrap_err()
            .code(),
        ErrorCode::ObjectCorrupt
    );
    assert_eq!(
        snapshot_receipt(&engine, &members, &snapshot, last.operation_id, true)
            .unwrap()
            .unwrap()
            .result,
        last
    );
}

#[test]
fn published_replay_reports_truncated_locator_or_segment_as_corrupt_evidence() {
    use std::error::Error as _;
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([
            temp.path().join("a"),
            temp.path().join("b"),
            temp.path().join("c"),
        ]),
    )
    .unwrap();
    let id = OperationId::generate();
    let padding = storage_contract::OPERATION_SEGMENT_TARGET_BYTES;
    let published = publish(&mut engine, id, padding).unwrap();
    publish(&mut engine, OperationId::generate(), 0).unwrap();
    assert!(engine.state.operations.fresh_seal().is_none());
    assert!(engine.state.operations.cold_index().is_none());
    assert!(engine.state.operations.tail_record(id).is_none());
    let missing = OperationId::generate();
    assert!(engine.find_operation_lookup(missing).unwrap().is_none());
    let locator = engine.operation_locator_root.as_ref().unwrap();
    let addresses = [
        (locator.kind().domain(), locator.hash().to_owned()),
        (
            storage_contract::OPERATION_SEGMENT_KIND,
            engine.state.operations.sealed()[0].hash.clone(),
        ),
    ];
    let members = engine.members();
    let sequence = engine.state.sequence;
    for (kind, hash) in addresses {
        let originals: Vec<_> = members
            .iter()
            .map(|member| {
                let path = crate::journal::object_path(member, kind, &hash);
                let bytes = std::fs::read(&path).unwrap();
                std::fs::write(&path, b"{truncated").unwrap();
                (path, bytes)
            })
            .collect();
        engine.segment_caches = crate::segment_store::SegmentCaches::new();
        let error = publish(&mut engine, id, padding).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
        let mut source = error.source();
        let mut json_cause = false;
        while let Some(cause) = source {
            json_cause |= cause.is::<json_codec::Error>();
            source = cause.source();
        }
        assert!(json_cause, "stored JSON failure retains its concrete cause");
        assert_eq!(engine.state.sequence, sequence);
        for (path, bytes) in originals {
            std::fs::write(path, bytes).unwrap();
        }
        engine.segment_caches = crate::segment_store::SegmentCaches::new();
        assert_eq!(publish(&mut engine, id, padding).unwrap(), published);
        assert!(engine.find_operation_lookup(missing).unwrap().is_none());
    }
}

#[test]
fn pending_locator_semantics_keep_storage_context_and_original_sources() {
    use std::error::Error as _;
    let (_temp, engine) = fixture();
    let members = engine.members();
    let id = OperationId::generate();
    for value in [
        json_codec::json!(42),
        json_codec::json!("18446744073709551616"),
    ] {
        let json_type_error = value.is_number();
        let mut snapshot =
            crate::journal::prepare_snapshot(&State::default(), crate::disk::MAX_METADATA)
                .unwrap()
                .descriptor;
        snapshot.maps.insert(
            "operation_locators".into(),
            crate::journal::stored_map(&members, [(id.to_string(), value)].into_iter().collect()),
        );
        let error = snapshot_receipt(&engine, &members, &snapshot, id, false).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
        let mut source = error.source();
        let mut found = false;
        while let Some(cause) = source {
            found |= if json_type_error {
                cause.is::<json_codec::Error>()
            } else {
                cause.is::<std::num::ParseIntError>()
            };
            source = cause.source();
        }
        assert!(found);
    }
}
