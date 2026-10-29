use super::*;
use crate::{
    EVENT_SEGMENT_SPAN, Event, OPERATION_SEGMENT_RECORDS, OperationId, OperationRecordEntry,
    OperationResult, SegmentChanges, SegmentedEvents,
};
use std::collections::BTreeSet;
use std::sync::Arc;

fn event(index: u64) -> Event {
    Event {
        sequence: index,
        operation_id: format!("op_{index:032x}").parse().unwrap(),
        kind: "MUTATION".into(),
        object_id: "object".into(),
    }
}
fn receipt(index: u64) -> OperationRecordEntry {
    OperationRecordEntry {
        principal: "owner".into(),
        result: OperationResult {
            operation_id: format!("op_{index:032x}").parse().unwrap(),
            kind: "file.save".into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: crate::CommitState::Published,
            protection_at_publish: crate::ProtectionAtPublish::NotApplicable,
            current_health: crate::CurrentHealth::Healthy,
            request_digest: "digest".into(),
            sequence: index,
            result: json_codec::json!({}),
        },
    }
}

#[test]
fn event_pushes_seal_exactly_at_the_span_and_track_dirty_segment_ordinals() {
    let mut log = SegmentedEvents::new();
    let mut before = log.clone();
    before.begin_tracking();
    for index in 0..EVENT_SEGMENT_SPAN as u64 - 1 {
        log.push(event(index)).unwrap();
    }
    assert!(log.sealed().is_empty());
    assert_eq!(log.len(), EVENT_SEGMENT_SPAN as u64 - 1);
    let full = {
        let mut full = log.clone();
        full.begin_tracking();
        full.push(event(EVENT_SEGMENT_SPAN as u64 - 1)).unwrap();
        full
    };
    assert_eq!(full.sealed().len(), 1);
    assert!(full.tail().is_empty());
    assert_eq!(full.sealed()[0].first_sequence, 0);
    let (ordinal, segment) = full.fresh_seal().unwrap();
    assert_eq!(ordinal, 0);
    assert_eq!(segment.events.len(), EVENT_SEGMENT_SPAN);
    assert_eq!(
        crate::object_hash(crate::EVENT_SEGMENT_KIND, segment).unwrap(),
        full.sealed()[0].hash
    );
    // The sealing append dirties the segment that sealed; the empty new tail
    // adds no key. A later append dirties only the next ordinal.
    assert_eq!(
        full.changes_since(&log),
        SegmentChanges::Segments(Arc::new(BTreeSet::from([0])))
    );
    let mut next = full.clone();
    next.begin_tracking();
    assert!(
        next.fresh_seal().is_none(),
        "fresh seals are one transaction's"
    );
    next.push(event(EVENT_SEGMENT_SPAN as u64)).unwrap();
    assert_eq!(
        next.changes_since(&full),
        SegmentChanges::Segments(Arc::new(BTreeSet::from([1])))
    );
    // A foreign rebuild loses lineage.
    let rebuilt =
        SegmentedEvents::from_events((0..EVENT_SEGMENT_SPAN as u64 + 1).map(event).collect())
            .unwrap();
    assert_eq!(rebuilt, next);
    assert_eq!(rebuilt.changes_since(&full), SegmentChanges::All);
}

#[test]
fn operation_pushes_seal_at_the_record_count_or_the_byte_target_whichever_first() {
    let mut log = crate::OperationLog::new();
    for index in 0..OPERATION_SEGMENT_RECORDS as u64 {
        log.push(receipt(index)).unwrap();
    }
    assert_eq!(log.sealed().len(), 1);
    assert_eq!(log.sealed()[0].records, OPERATION_SEGMENT_RECORDS as u64);
    assert_eq!(log.sealed()[0].first_ordinal, 0);
    assert_eq!(log.tail_first_ordinal(), OPERATION_SEGMENT_RECORDS as u64);
    assert!(log.tail().is_empty());
    // One oversized receipt crosses the byte target alone and seals a
    // single-record segment.
    let mut big = receipt(OPERATION_SEGMENT_RECORDS as u64);
    big.result.result = json_codec::json!({
        "payload": "字".repeat(crate::OPERATION_SEGMENT_TARGET_BYTES / 3)
    });
    log.push(big).unwrap();
    assert_eq!(log.sealed().len(), 2);
    assert_eq!(log.sealed()[1].records, 1);
    assert_eq!(
        log.sealed()[1].first_ordinal,
        OPERATION_SEGMENT_RECORDS as u64
    );
    // The cold index stays complete across seals until released.
    let index = log.cold_index().unwrap();
    assert_eq!(index.len() as u64, log.len());
    for ordinal in 0..log.len() {
        let id: OperationId = format!("op_{ordinal:032x}").parse().unwrap();
        assert_eq!(index.get(&id), Some(&ordinal));
    }
}

#[test]
fn appended_ids_cover_resident_records_and_lost_lineage_returns_none() {
    let mut before = crate::OperationLog::new();
    for index in 0..OPERATION_SEGMENT_RECORDS as u64 - 1 {
        before.push(receipt(index)).unwrap();
    }
    let mut after = before.clone();
    after.begin_tracking();
    // Crossing the seal keeps every appended id visible through the retained
    // fresh seal plus the new tail.
    after
        .push(receipt(OPERATION_SEGMENT_RECORDS as u64))
        .unwrap();
    after
        .push(receipt(OPERATION_SEGMENT_RECORDS as u64 + 1))
        .unwrap();
    let appended = after.appended_ids_since(&before).unwrap();
    assert_eq!(
        appended,
        vec![
            (
                format!("op_{:032x}", OPERATION_SEGMENT_RECORDS as u64)
                    .parse()
                    .unwrap(),
                OPERATION_SEGMENT_RECORDS as u64 - 1
            ),
            (
                format!("op_{:032x}", OPERATION_SEGMENT_RECORDS as u64 + 1)
                    .parse()
                    .unwrap(),
                OPERATION_SEGMENT_RECORDS as u64
            ),
        ]
    );
    let rebuilt = crate::OperationLog::from_records(before.tail().to_vec()).unwrap();
    assert!(rebuilt.appended_ids_since(&before).is_none());
}

#[test]
fn tail_updates_rebind_the_newest_receipt_and_reseal_deterministically() {
    let mut log = crate::OperationLog::new();
    for index in 0..3 {
        log.push(receipt(index)).unwrap();
    }
    // A plain tail update targets the newest receipt only.
    let mut corrected = receipt(2);
    corrected.result.commit_state = crate::CommitState::PublishedDegraded;
    log.replace_last(corrected.clone()).unwrap();
    assert_eq!(log.tail().last().unwrap(), &corrected);
    let mut wrong = receipt(7);
    wrong.result.operation_id = "op_0000000000000000000000000000dead".parse().unwrap();
    assert_eq!(
        log.replace_last(wrong).unwrap_err().code(),
        crate::ErrorCode::StateInvalid
    );
    // At a seal boundary the exact sealed body reopens, updates, and
    // re-seals under a new hash; foreign bytes fail closed.
    let mut log = crate::OperationLog::from_records(
        (0..OPERATION_SEGMENT_RECORDS as u64).map(receipt).collect(),
    )
    .unwrap();
    assert!(log.tail().is_empty());
    let sealed = log.sealed()[0].clone();
    let before = log.clone();
    log.begin_tracking();
    let mut forged = (0..OPERATION_SEGMENT_RECORDS as u64)
        .map(receipt)
        .collect::<Vec<_>>();
    forged[0].principal = "forged".into();
    assert_eq!(
        log.reopen_last_sealed(forged).unwrap_err().code(),
        crate::ErrorCode::ObjectCorrupt
    );
    log.reopen_last_sealed((0..OPERATION_SEGMENT_RECORDS as u64).map(receipt).collect())
        .unwrap();
    assert!(log.sealed().is_empty());
    let mut corrected = receipt(OPERATION_SEGMENT_RECORDS as u64 - 1);
    corrected.result.commit_state = crate::CommitState::PublishedDegraded;
    log.replace_last(corrected.clone()).unwrap();
    assert_eq!(log.sealed().len(), 1);
    assert!(log.tail().is_empty());
    assert_ne!(log.sealed()[0].hash, sealed.hash);
    assert_eq!(log.sealed()[0].records, sealed.records);
    let (_, segment) = log.fresh_seal().unwrap();
    assert_eq!(segment.records.last().unwrap(), &corrected);
    assert_eq!(log.changes_since(&before), crate::SegmentChanges::All);
}

#[test]
fn reattached_cold_indexes_bind_to_the_resident_log_or_fail_closed() {
    let mut decoded: crate::OperationLog = json_codec::from_value(
        json_codec::to_value(
            &crate::OperationLog::from_records(
                (0..OPERATION_SEGMENT_RECORDS as u64 + 2)
                    .map(receipt)
                    .collect(),
            )
            .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(decoded.cold_index().is_none());
    let id = |ordinal: u64| -> OperationId { format!("op_{ordinal:032x}").parse().unwrap() };
    let complete: BTreeMap<OperationId, u64> = (0..decoded.len())
        .map(|ordinal| (id(ordinal), ordinal))
        .collect();
    // Incomplete, out-of-range, duplicated and mis-bound indexes fail closed.
    let mut short = complete.clone();
    short.remove(&id(0));
    assert_eq!(
        decoded.attach_cold_index(short).unwrap_err().code(),
        crate::ErrorCode::ObjectCorrupt
    );
    let mut outside = complete.clone();
    outside.insert(id(0), decoded.len());
    assert_eq!(
        decoded.attach_cold_index(outside).unwrap_err().code(),
        crate::ErrorCode::ObjectCorrupt
    );
    let mut duplicated = complete.clone();
    duplicated.insert(id(0), 1);
    assert_eq!(
        decoded.attach_cold_index(duplicated).unwrap_err().code(),
        crate::ErrorCode::ObjectCorrupt
    );
    let last = decoded.len() - 1;
    let mut swapped_tail = complete.clone();
    swapped_tail.insert(id(last), last - 1);
    swapped_tail.insert(id(last - 1), last);
    assert_eq!(
        decoded.attach_cold_index(swapped_tail).unwrap_err().code(),
        crate::ErrorCode::ObjectCorrupt
    );
    decoded.attach_cold_index(complete).unwrap();
    assert_eq!(decoded.cold_index().unwrap().len() as u64, decoded.len());
    assert_eq!(decoded.cold_index().unwrap().get(&id(last)), Some(&last));
    // A retained fresh seal re-checks its resident sealed records exactly;
    // non-resident sealed entries carry the published root's hash authority.
    let mut live = crate::OperationLog::from_records(
        (0..OPERATION_SEGMENT_RECORDS as u64).map(receipt).collect(),
    )
    .unwrap();
    assert!(live.fresh_seal().is_some());
    live.release_cold_index();
    let complete: BTreeMap<OperationId, u64> = (0..live.len())
        .map(|ordinal| (id(ordinal), ordinal))
        .collect();
    let mut swapped_seal = complete.clone();
    swapped_seal.insert(id(0), 1);
    swapped_seal.insert(id(1), 0);
    assert_eq!(
        live.attach_cold_index(swapped_seal).unwrap_err().code(),
        crate::ErrorCode::ObjectCorrupt
    );
    live.attach_cold_index(complete).unwrap();
    assert_eq!(live.cold_index().unwrap().len() as u64, live.len());
}

#[test]
fn wire_shapes_round_trip_and_sealed_history_requires_the_attached_index() {
    let log = crate::OperationLog::from_records((0..5).map(receipt).collect()).unwrap();
    let value = json_codec::to_value(&log).unwrap();
    // The serialized shape carries no index; without sealed history the
    // decode derives the complete index from the tail alone.
    assert!(value.get("index").is_none());
    let decoded: crate::OperationLog = json_codec::from_value(value).unwrap();
    assert_eq!(decoded, log);
    assert_eq!(decoded.cold_index().unwrap().len(), 5);
    let sealed = crate::OperationLog::from_records(
        (0..OPERATION_SEGMENT_RECORDS as u64 + 1)
            .map(receipt)
            .collect(),
    )
    .unwrap();
    let decoded: crate::OperationLog =
        json_codec::from_value(json_codec::to_value(&sealed).unwrap()).unwrap();
    assert_eq!(decoded, sealed);
    // Sealed bodies are not resident after a plain wire decode, so the
    // complete id index must come from the disk-read entry.
    assert!(decoded.cold_index().is_none());
    let events = SegmentedEvents::from_events((0..3).map(event).collect()).unwrap();
    let decoded: SegmentedEvents =
        json_codec::from_value(json_codec::to_value(&events).unwrap()).unwrap();
    assert_eq!(decoded, events);
}
