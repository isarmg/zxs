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
    for _ in 0..storage_contract::OPERATION_SEGMENT_RECORDS + 1 {
        published.push(publish(&mut engine, OperationId::generate(), 0).unwrap());
    }
    assert_eq!(engine.state.operations.sealed().len(), 1);
    assert_eq!(engine.state.operations.tail().len(), 1);
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
