use crate::files::{
    FileOperation, SaveStatus,
    deletion::{
        outcome_endpoint,
        recovery::{Queue, Record, Retained},
        request::{Attempt, Intent},
    },
};
use crate::ui::State;
use json_codec::{Value, json};
use std::sync::Arc;
use storage_contract::file_deletion_review::Summary;

fn attempt() -> Attempt {
    let summary: Summary = json_codec::from_value(json!({
        "file_id":"file_11111111111111111111111111111111",
        "node_id":"fdel_11111111111111111111111111111111",
        "repository_id":"repo_11111111111111111111111111111111",
        "history_tree_id":"history_11111111111111111111111111111111",
        "path":"large.txt", "operation_id":"op_11111111111111111111111111111111",
        "author":"alice", "sequence":"1", "created_at":"123",
        "branch_count":"65536", "parent_count":"65536"
    }))
    .unwrap();
    let intent = Intent::UndoSummary(Arc::new(summary));
    Attempt {
        epoch: 1,
        principal: "alice".into(),
        operation: "op_22222222222222222222222222222222".into(),
        request_digest: intent.digest("alice").unwrap(),
        intent,
        in_flight: false,
    }
}
fn receipt(attempt: &Attempt) -> Value {
    let Intent::UndoSummary(summary) = &attempt.intent else {
        panic!("expected a summary undo fixture");
    };
    json!({
        "operation_id":attempt.operation, "operation_kind":"file.delete.undo",
        "request_digest":attempt.request_digest, "state":"completed", "commit_state":"PUBLISHED",
        "publication_sequence":"2", "protection_at_publish":"NOT_APPLICABLE", "error":null,
        "result":{
            "restored_file_id":summary.file_id, "removed_deletion_summary":summary.as_ref(),
            "timestamp":"124", "membership_epoch":"1", "version_protection":[]
        }
    })
}
#[test]
fn summary_original_request_survives_refresh_without_retaining_the_frontier() {
    let attempt = attempt();
    let record = Record::from_attempt(&attempt).unwrap();
    assert!(record.bytes.len() < 2048);
    let stored: Value = json_codec::from_str(&record.bytes).unwrap();
    assert_eq!(stored["request"]["kind"], "undo_summary");
    assert!(stored["request"].get("deletion_node").is_none());
    let restored = Record::decode(&record.key(), "alice", &record.bytes).unwrap();
    assert_eq!(restored.intent, attempt.intent);
    assert_eq!(restored.request_digest, attempt.request_digest);
    assert_eq!(restored.operation.to_string(), attempt.operation);
    assert_eq!(restored.bytes, record.bytes);
    let resumed = restored.attempt(9);
    assert_eq!(resumed.epoch, 9);
    assert!(!resumed.in_flight);
    assert_eq!(resumed.operation, attempt.operation);
}
#[test]
fn restored_summary_operation_uses_only_the_original_compact_outcome_route() {
    let attempt = attempt();
    let record = Record::from_attempt(&attempt).unwrap();
    let restored = Record::decode(&record.key(), "alice", &record.bytes).unwrap();
    let mut state = State::default();
    state.authenticated = true;
    state.principal = "alice".into();
    state.session_epoch = 3;
    let mut queue = Queue::default();
    queue.owner = Some((3, "alice".into()));
    queue.merge(vec![Retained::unknown(restored)]).unwrap();
    state.files.deletion.recovery = queue;
    state
        .files
        .operations
        .push(FileOperation::unknown(&attempt.operation));
    assert_eq!(
        outcome_endpoint(&state, &attempt.operation).unwrap(),
        format!("/api/file-deletion-undos/{}", attempt.operation)
    );
    assert!(outcome_endpoint(&state, "op_33333333333333333333333333333333").is_none());
    assert!(state.files.deletion.attempt.is_none());
    assert!(!state.files.mutation_pending);
    assert!(crate::files::deletion::blocks_writes(&state));
    state.session_epoch += 1;
    assert!(outcome_endpoint(&state, &attempt.operation).is_none());
}
#[test]
fn unknown_partial_or_foreign_receipts_cannot_clear_a_summary_recovery_record() {
    let attempt = attempt();
    let record = Record::from_attempt(&attempt).unwrap();
    let correct = receipt(&attempt);
    assert!(record.matches_receipt(&attempt.operation, &correct));
    for (field, changed) in [
        ("request_digest", json!("f".repeat(64))),
        ("operation_id", json!("op_33333333333333333333333333333333")),
        ("operation_kind", json!("file.delete.batch.undo")),
        ("publication_sequence", json!("1")),
        ("commit_state", json!("IN_DOUBT")),
    ] {
        let mut wrong = correct.clone();
        wrong[field] = changed;
        assert!(!record.matches_receipt(&attempt.operation, &wrong));
    }
    let mut partial = correct.clone();
    partial["result"]["removed_deletion_summary"]
        .as_object_mut()
        .unwrap()
        .remove("parent_count");
    assert!(!record.matches_receipt(&attempt.operation, &partial));
    let mut changed = correct;
    changed["result"]["removed_deletion_summary"]["node_id"] =
        json!("fdel_33333333333333333333333333333333");
    assert!(!record.matches_receipt(&attempt.operation, &changed));
    let mut queue = Queue::default();
    queue.merge(vec![Retained::unknown(record)]).unwrap();
    assert!(!queue.settle(&attempt.operation, SaveStatus::Unknown));
    assert_eq!(queue.records.len(), 1);
    assert!(queue.records[0].settled.is_none());
}
#[test]
fn damaged_summary_storage_preserves_original_bytes_and_account_binding() {
    let record = Record::from_attempt(&attempt()).unwrap();
    assert!(Record::decode(&record.key(), "bob", &record.bytes).is_err());
    let valid: Value = json_codec::from_str(&record.bytes).unwrap();
    let mut missing = valid.clone();
    missing["request"]["deletion_summary"]
        .as_object_mut()
        .unwrap()
        .remove("node_id");
    let mut wrong_type = valid.clone();
    wrong_type["request"]["deletion_summary"]["branch_count"] = json!(65536);
    let mut array = valid.clone();
    let summary = &valid["request"]["deletion_summary"];
    array["request"]["deletion_summary"] = json!([
        summary["file_id"], summary["node_id"], summary["repository_id"], summary["history_tree_id"],
        summary["path"], summary["operation_id"], summary["author"], summary["sequence"],
        summary["created_at"], summary["branch_count"], summary["parent_count"]
    ]);
    for invalid in [missing, wrong_type, array] {
        let raw = json_codec::to_string(&invalid).unwrap();
        let retained = raw.clone();
        assert!(Record::decode(&record.key(), "alice", &raw).is_err());
        assert_eq!(raw, retained);
    }
    let duplicate = record.bytes.replace(
        "\"node_id\":",
        "\"branch_count\":\"1\",\"node_id\":",
    );
    assert!(Record::decode(&record.key(), "alice", &duplicate).is_err());
}

#[test]
fn late_compact_receipt_requires_the_same_session_and_keeps_the_original_unknown_fence() {
    let attempt = attempt();
    let record = Record::from_attempt(&attempt).unwrap();
    let mut state = State::default();
    state.authenticated = true;
    state.principal = "alice".into();
    state.session_epoch = attempt.epoch;
    let mut queue = Queue::default();
    queue.owner = Some((attempt.epoch, "alice".into()));
    queue.merge(vec![Retained::unknown(record)]).unwrap();
    state.files.deletion.recovery = queue;
    state
        .files
        .operations
        .push(FileOperation::unknown(&attempt.operation));
    let value = receipt(&attempt);
    assert!(crate::files::deletion::receipt_matches(
        &state,
        &attempt.operation,
        &value,
    ));
    state.files.deletion.close();
    assert!(crate::files::deletion::receipt_matches(
        &state,
        &attempt.operation,
        &value,
    ));
    assert!(outcome_endpoint(&state, &attempt.operation).is_some());
    state.session_epoch += 1;
    assert!(!crate::files::deletion::receipt_matches(
        &state,
        &attempt.operation,
        &value,
    ));
    assert_eq!(state.files.operations[0].status(), SaveStatus::Unknown);
    assert_eq!(state.files.deletion.recovery.records.len(), 1);
    assert!(state.files.deletion.attempt.is_none());
}
