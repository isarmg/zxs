use super::*;

fn event(sequence: u64) -> Event {
    Event {
        sequence,
        operation_id: format!("op_{sequence:032x}").parse().unwrap(),
        kind: "fixture".into(),
        object_id: "fixture".into(),
    }
}

#[test]
fn incremental_directory_encoding_visits_dirty_leaf_values_not_the_sealed_prefix() {
    // Equal-sequence events are valid; repeated immutable bodies let this
    // fixture exercise a large directory without materializing large history.
    let segment = storage_contract::EventSegment {
        format: Default::default(),
        events: vec![event(0); storage_contract::EVENT_SEGMENT_SPAN],
    };
    let reference = storage_contract::EventSegmentRef {
        hash: object_hash(EVENT_SEGMENT_KIND, &segment).unwrap(),
        first_sequence: 0,
    };
    let events = json_codec::from_value(json_codec::json!({
        "sealed": vec![reference; 512], "tail": [event(0)],
    }))
    .unwrap();
    let receipts = (0..4096u64)
        .map(|sequence| storage_contract::OperationRecordEntry {
            principal: "owner".into(),
            result: OperationResult {
                operation_id: event(sequence).operation_id,
                kind: "fixture".into(),
                commit_id: None,
                history_tree_id: None,
                branch_id: None,
                commit_state: storage_contract::CommitState::Published,
                protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
                current_health: storage_contract::CurrentHealth::Healthy,
                request_digest: "digest".into(),
                sequence,
                result: json_codec::json!({}),
            },
        })
        .collect();
    let before = State {
        sequence: 4096,
        events,
        operations: storage_contract::OperationLog::from_records(receipts).unwrap(),
        ..Default::default()
    };
    let base = prepare(&before, MAX_METADATA).unwrap();
    let mut after = before.clone();
    after.begin_tracking();
    after.sequence += 1;
    after.events.push(event(after.sequence)).unwrap();
    SEGMENT_DIRECTORY_SERIALIZATIONS.with(|counts| counts.borrow_mut().clear());
    let incremental = prepare_incremental(
        &before,
        &after,
        &base.reuse_basis(),
        MAX_METADATA,
        &mut no_reused_sources,
    )
    .unwrap();
    let counts = SEGMENT_DIRECTORY_SERIALIZATIONS.with(|counts| counts.borrow().clone());
    assert_eq!(counts.get("operation_log").copied().unwrap_or(0), 0);
    assert!(counts["events"] > 0 && counts["events"] < 128, "{counts:?}");
    assert_eq!(
        incremental.hash,
        prepare(&after, MAX_METADATA).unwrap().hash
    );
}
