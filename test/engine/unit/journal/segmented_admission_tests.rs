use super::*;
use storage_contract::{
    EVENT_SEGMENT_KIND, EVENT_SEGMENT_SPAN, EventDirectoryEntry, EventSegment,
    OPERATION_SEGMENT_KIND, OPERATION_SEGMENT_RECORDS, OPERATION_SEGMENT_TARGET_BYTES,
    OperationDirectoryEntry, OperationRecordEntry, OperationSegment,
};

fn receipt(sequence: u64) -> OperationRecordEntry {
    OperationRecordEntry {
        principal: "owner".into(),
        result: OperationResult {
            operation_id: format!("op_{sequence:032x}").parse().unwrap(),
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
    }
}

#[test]
fn segment_shape_decodes_every_record_before_retaining_facts() {
    let segment = OperationSegment {
        format: Default::default(),
        records: vec![receipt(1), receipt(2)],
    };
    for field in ["operation_id", "unknown_field"] {
        let mut value = json_codec::to_value(&segment).unwrap();
        value["records"][1]["result"][field] = "invalid".into();
        let hash = object_hash(OPERATION_SEGMENT_KIND, &value).unwrap();
        let object =
            AuthenticatedMetadataObject::authenticate(OPERATION_SEGMENT_KIND, &hash, value)
                .unwrap();
        assert!(SegmentShape::read(&object).is_err());
    }
}

fn read_receipts(records: Vec<OperationRecordEntry>, sequence: u64) -> Result<json_codec::Value> {
    let locators = records
        .iter()
        .enumerate()
        .map(|(ordinal, record)| {
            (
                record.result.operation_id.to_string(),
                ordinal.to_string().into(),
            )
        })
        .collect();
    let segment = OperationSegment {
        format: Default::default(),
        records,
    };
    let hash = object_hash(OPERATION_SEGMENT_KIND, &segment)?;
    let entry = OperationDirectoryEntry {
        kind: Default::default(),
        hash: hash.clone(),
        first_sequence: segment.records[0].result.sequence,
        first_ordinal: 0,
        records: segment.records.len() as u64,
    };
    resolve_operation_segments(
        [(format!("{:020}", 0), json_codec::to_value(entry)?)]
            .into_iter()
            .collect(),
        locators,
        sequence,
        &mut |kind, address| {
            assert_eq!((kind, address), (OPERATION_SEGMENT_KIND, hash.as_str()));
            AuthenticatedMetadataObject::authenticate(
                kind,
                address,
                json_codec::to_value(&segment)?,
            )
        },
    )
}

#[test]
fn fully_sealed_logs_reject_future_sequences_before_discarding_the_bodies() {
    let events = (0..EVENT_SEGMENT_SPAN as u64)
        .map(|sequence| Event {
            sequence,
            operation_id: receipt(sequence).result.operation_id,
            kind: "fixture".into(),
            object_id: "fixture".into(),
        })
        .collect();
    let segment = EventSegment {
        format: Default::default(),
        events,
    };
    let hash = object_hash(EVENT_SEGMENT_KIND, &segment).unwrap();
    let directory = [(
        format!("{:020}", 0),
        json_codec::to_value(EventDirectoryEntry {
            kind: Default::default(),
            hash: hash.clone(),
            first_sequence: 0,
        })
        .unwrap(),
    )]
    .into_iter()
    .collect();
    let error = resolve_event_segments(directory, 0, &mut |kind, address| {
        AuthenticatedMetadataObject::authenticate(kind, address, json_codec::to_value(&segment)?)
    })
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::StateInvalid);
    assert_eq!(
        error.message(),
        "snapshot event sequence exceeds published sequence"
    );
    let error = read_receipts(
        (0..OPERATION_SEGMENT_RECORDS as u64).map(receipt).collect(),
        0,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::StateInvalid);
    assert_eq!(
        error.message(),
        "snapshot operation sequence exceeds published sequence"
    );
}

#[test]
fn operation_segments_end_at_the_first_seal_threshold() {
    let records = (0..OPERATION_SEGMENT_RECORDS as u64)
        .map(receipt)
        .collect::<Vec<_>>();
    assert!(read_receipts(records.clone(), u64::MAX).is_ok());
    let mut extra = records;
    extra.push(receipt(OPERATION_SEGMENT_RECORDS as u64));
    let error = read_receipts(extra, u64::MAX).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
    let mut oversized = receipt(0);
    oversized.result.result =
        json_codec::json!({"padding": "x".repeat(OPERATION_SEGMENT_TARGET_BYTES)});
    assert!(read_receipts(vec![oversized.clone()], u64::MAX).is_ok());
    let error = read_receipts(vec![oversized, receipt(1)], u64::MAX).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
    assert_eq!(
        error.message(),
        "operation segment continues beyond the seal rule"
    );
}
