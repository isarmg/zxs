//! Source-only cases for browser hint admission and original-operation ownership.
//! Browser storage/DOM execution is intentionally not claimed by these cases.
use super::*;

fn record() -> Record {
    Record {
        format: Format::Request,
        principal: "admin".into(),
        operation: format!("op_{}", "a".repeat(32)).parse().unwrap(),
        file_id: HistoryFileId::parse(&format!("file_{}", "b".repeat(32))).unwrap(),
        history_root: HistoryRoot::parse(&"c".repeat(64)).unwrap(),
        version_ids: vec![HistoryVersionId::parse(&format!("ver_{}", "d".repeat(32))).unwrap()],
        label: "detached/report.txt".into(),
    }
}
fn authenticated(record: &Record, epoch: u64) -> State {
    let mut state = State {
        authenticated: true, principal: record.principal.clone(), session_epoch: epoch,
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state.files.node_retirement.recovery = Queue {
        owner: Some((epoch, record.principal.clone())), records: vec![record.clone()], error: None,
    };
    state
}
fn receipt(record: &Record) -> Value {
    json!({
        "operation_id": record.operation,
        "operation_kind": "file.nodes.retire",
        "request_digest": "e".repeat(64),
        "state": "completed", "commit_state": "PUBLISHED",
        "protection_at_publish": "NOT_APPLICABLE", "publication_sequence": "12",
        "error": null,
        "result": {"retirement": {"request": {
            "origin_file_id": record.file_id,
            "history_root": record.history_root,
            "version_ids": record.version_ids,
        }}},
    })
}

#[test]
fn saved_request_round_trip_keeps_exact_identity_but_never_restores_success() {
    let original = record();
    let bytes = json_codec::to_string(&original).unwrap();
    let saved = Record::decode(&original.key(), "admin", &bytes).unwrap();
    assert_eq!(saved, original);
    let attempt = saved.restored_attempt(8).unwrap();
    assert_eq!(attempt.selection, original.selection().unwrap());
    assert_eq!(attempt.operation, original.operation.to_string());
    assert_eq!(Record::from_attempt(&attempt), original);
    assert!(attempt.uncertain && attempt.restored);
    assert!(!attempt.in_flight && !attempt.retry_confirmed);
    assert!(attempt.cleanup.is_none() && attempt.separation.is_none());
    assert_eq!(status(&authenticated(&original, 8), &attempt), SaveStatus::Unknown);
    assert!(!bytes.contains("token") && !bytes.contains("cleanup") && !bytes.contains("password"));
}

#[test]
fn storage_boundary_rejects_other_accounts_keys_extra_fields_and_unsupported_formats() {
    let original = record();
    let bytes = json_codec::to_string(&original).unwrap();
    assert!(Record::decode(&original.key(), "another-admin", &bytes).is_err());
    assert!(Record::decode(&format!("{}other", original.key()), "admin", &bytes).is_err());
    for (field, value) in [
        ("format", json!("old-node-retirement")),
        ("cleanup", json!({"state":"observed"})),
        ("operation", json!("not-an-operation")),
        ("version_ids", json!([])),
        ("version_ids", json!([original.version_ids[0], original.version_ids[0]])),
        ("label", json!("x".repeat(MAX_LABEL_BYTES + 1))),
    ] {
        // Each invalid value is independent of the immutable source record.
        let mut changed = json_codec::to_value(&original).unwrap();
        changed[field] = value;
        assert!(Record::decode(&original.key(), "admin", &json_codec::to_string(&changed).unwrap()).is_err());
    }
    assert!(Record::decode(&original.key(), "admin", &" ".repeat(wire::MAX_REQUEST_BYTES + 1)).is_err());
    let duplicate = bytes.replacen("{", "{\"principal\":\"admin\",", 1);
    assert!(Record::decode(&original.key(), "admin", &duplicate).is_err());
    assert_eq!(json_codec::to_string(&original).unwrap(), bytes);
}

#[test]
fn restored_unknown_request_requires_explicit_confirmation_and_current_owner_not_a_listed_file() {
    let saved = record();
    let mut state = authenticated(&saved, 8);
    let mut attempt = saved.restored_attempt(8).unwrap();
    assert!(state.files.selected.is_none());
    assert!(!attempt.retry_allowed(&state));
    attempt.retry_confirmed = true;
    assert!(attempt.retry_allowed(&state));
    state.session_epoch = 9;
    assert!(!attempt.retry_allowed(&state));
    state.session_epoch = 8;
    state.principal = "another-admin".into();
    assert!(!attempt.retry_allowed(&state));
    state.principal = saved.principal.clone();
    state.files.mutation_pending = true;
    assert!(!attempt.retry_allowed(&state));
}

#[test]
fn every_retained_request_is_matched_even_when_a_different_recovery_row_is_open() {
    let saved = record();
    let mut other = saved.clone();
    other.operation = format!("op_{}", "f".repeat(32)).parse().unwrap();
    let mut state = authenticated(&saved, 8);
    state.files.node_retirement.recovery.records.push(other.clone());
    state.files.node_retirement.attempt = Some(other.restored_attempt(8).unwrap());
    let exact = receipt(&saved);
    assert!(super::super::receipt_matches(&state, &saved.operation.to_string(), &exact));
    for field in ["origin_file_id", "history_root", "version_ids"] {
        let mut changed = exact.clone();
        changed["result"]["retirement"]["request"][field] = Value::Null;
        assert!(!super::super::receipt_matches(&state, &saved.operation.to_string(), &changed));
    }
    state.principal = "another-admin".into();
    assert!(!super::super::receipt_matches(&state, &saved.operation.to_string(), &exact));
}

#[test]
fn verified_publication_allows_only_original_resume_without_restoring_old_cleanup() {
    let saved = record();
    let mut state = authenticated(&saved, 8);
    let operation = saved.operation.to_string();
    state.files.operations.push(FileOperation::from_receipt(&operation, &receipt(&saved)));
    let attempt = saved.restored_attempt(8).unwrap();
    assert!(published(status(&state, &attempt)));
    assert!(!attempt.retry_confirmed && attempt.retry_allowed(&state));
    assert!(attempt.cleanup.is_none());
    assert!(!state.files.node_retirement.recovery.ready_for_new(&state));
    state.files.operations = vec![FileOperation::rejected(&operation)];
    assert!(!attempt.retry_allowed(&state));
}

#[test]
fn stale_operation_controls_do_not_retarget_a_reauthenticated_attempt() {
    let saved = record();
    Owner::new().with(|| {
        let mut snapshot = authenticated(&saved, 9);
        let mut attempt = saved.restored_attempt(9).unwrap();
        attempt.retry_confirmed = true;
        snapshot.files.node_retirement.attempt = Some(attempt);
        let state = RwSignal::new(snapshot);
        // These stale calls must return before touching browser storage or I/O.
        super::super::action::retry(state, &saved.operation.to_string(), 8);
        super::super::action::dismiss(state, &saved.operation.to_string(), 8);
        open(state, saved.operation, 8);
        state.with_untracked(|state| {
            let attempt = state.files.node_retirement.attempt.as_ref().unwrap();
            assert_eq!(attempt.target.epoch, 9);
            assert!(attempt.retry_confirmed && !attempt.in_flight);
            assert!(!state.files.mutation_pending);
        });
    });
}
