//! Browser hint admission and original-operation ownership cases.
use super::*;

fn record() -> Record {
    Record {
        format: Format::Request,
        principal: "admin".parse().unwrap(),
        operation: format!("op_{}", "a".repeat(32)).parse().unwrap(),
        file_id: HistoryFileId::parse(&format!("file_{}", "b".repeat(32))).unwrap(),
        history_root: HistoryRoot::parse(&"c".repeat(64)).unwrap(),
        version_ids: vec![HistoryVersionId::parse(&format!("ver_{}", "d".repeat(32))).unwrap()],
        label: "detached/report.txt".into(),
    }
}
fn authenticated(record: &Record, epoch: u64) -> State {
    let mut state = State {
        authenticated: true,
        principal: Some(record.principal.clone()),
        session_epoch: epoch,
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state.files.node_retirement.recovery = Queue {
        owner: Some((epoch, record.principal.clone())),
        records: vec![record.clone()],
        error: None,
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
    let saved = Record::decode(&original.key(), &"admin".parse().unwrap(), &bytes).unwrap();
    assert_eq!(saved, original);
    let attempt = saved.restored_attempt(8).unwrap();
    assert_eq!(attempt.selection, original.selection().unwrap());
    assert_eq!(attempt.operation, original.operation.to_string());
    assert_eq!(Record::from_attempt(&attempt), original);
    assert!(attempt.uncertain && attempt.restored);
    assert!(!attempt.in_flight && !attempt.retry_confirmed);
    assert!(attempt.cleanup.is_none() && attempt.separation.is_none());
    assert_eq!(
        status(&authenticated(&original, 8), &attempt),
        SaveStatus::Unknown
    );
    assert!(!bytes.contains("token") && !bytes.contains("cleanup") && !bytes.contains("password"));
}

#[test]
fn storage_boundary_rejects_other_accounts_keys_extra_fields_and_unsupported_formats() {
    let original = record();
    let bytes = json_codec::to_string(&original).unwrap();
    assert!(Record::decode(&original.key(), &"another-admin".parse().unwrap(), &bytes).is_err());
    assert!(
        Record::decode(
            &format!("{}other", original.key()),
            &"admin".parse().unwrap(),
            &bytes,
        )
        .is_err()
    );
    for (field, value) in [
        ("format", json!("old-node-retirement")),
        ("cleanup", json!({"state":"observed"})),
        ("operation", json!("not-an-operation")),
        ("version_ids", json!([])),
        (
            "version_ids",
            json!([original.version_ids[0], original.version_ids[0]]),
        ),
        ("label", json!("x".repeat(MAX_LABEL_BYTES + 1))),
    ] {
        // Each invalid value is independent of the immutable source record.
        let mut changed = json_codec::to_value(&original).unwrap();
        changed[field] = value;
        assert!(
            Record::decode(
                &original.key(),
                &"admin".parse().unwrap(),
                &json_codec::to_string(&changed).unwrap()
            )
            .is_err()
        );
    }
    assert!(
        Record::decode(
            &original.key(),
            &"admin".parse().unwrap(),
            &" ".repeat(wire::MAX_REQUEST_BYTES + 1)
        )
        .is_err()
    );
    let duplicate = bytes.replacen("{", "{\"principal\":\"admin\",", 1);
    assert!(Record::decode(&original.key(), &"admin".parse().unwrap(), &duplicate).is_err());
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
    state.principal = Some("another-admin".parse().unwrap());
    assert!(!attempt.retry_allowed(&state));
    state.principal = Some(saved.principal.clone());
    state.files.mutation_pending = true;
    assert!(!attempt.retry_allowed(&state));
}

#[test]
fn every_retained_request_is_matched_even_when_a_different_recovery_row_is_open() {
    let saved = record();
    let mut other = saved.clone();
    other.operation = format!("op_{}", "f".repeat(32)).parse().unwrap();
    let mut state = authenticated(&saved, 8);
    state
        .files
        .node_retirement
        .recovery
        .records
        .push(other.clone());
    state.files.node_retirement.attempt = Some(other.restored_attempt(8).unwrap());
    let exact = receipt(&saved);
    assert!(super::super::receipt_matches(
        &state,
        &saved.operation.to_string(),
        &exact
    ));
    for field in ["origin_file_id", "history_root", "version_ids"] {
        let mut changed = exact.clone();
        changed["result"]["retirement"]["request"][field] = Value::Null;
        assert!(!super::super::receipt_matches(
            &state,
            &saved.operation.to_string(),
            &changed
        ));
    }
    state.principal = Some("another-admin".parse().unwrap());
    assert!(!super::super::receipt_matches(
        &state,
        &saved.operation.to_string(),
        &exact
    ));
}

#[test]
fn verified_publication_allows_only_original_resume_without_restoring_old_cleanup() {
    let saved = record();
    let mut state = authenticated(&saved, 8);
    let operation = saved.operation.to_string();
    state
        .files
        .operations
        .push(FileOperation::from_receipt(&operation, &receipt(&saved)));
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

#[test]
fn typed_retirement_principal_keeps_record_bytes_and_survives_absent_session() {
    let bytes = concat!(
        "{\"format\":\"file-node-retirement-recovery\",\"principal\":\" é/账号 \",",
        "\"operation\":\"op_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",",
        "\"file_id\":\"file_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",",
        "\"history_root\":\"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\",",
        "\"version_ids\":[\"ver_dddddddddddddddddddddddddddddddd\"],",
        "\"label\":\"detached/report.txt\"}",
    );
    let principal: Principal = " e\u{301}/账号 ".parse().unwrap();
    let key = format!("{}{}", prefix(&principal), record().operation);
    let saved = Record::decode(&key, &principal, bytes).unwrap();
    assert_eq!(saved.encode().unwrap(), bytes);
    let attempt = saved.restored_attempt(8).unwrap();
    assert_eq!(Record::from_attempt(&attempt).encode().unwrap(), bytes);
    let mut state = authenticated(&saved, 8);
    state.principal = None;
    assert!(owner(&state).is_none());
    assert!(!attempt.retry_allowed(&state));
    assert_eq!(state.files.node_retirement.recovery.records[0].principal, principal);
    state.principal = Some(principal.clone());
    state.session_epoch = 9;
    assert!(!attempt.retry_allowed(&state));
    assert_eq!(Record::from_attempt(&attempt).encode().unwrap(), bytes);
    for value in [json!(""), Value::Null, json!(0)] {
        let mut malformed: Value = json_codec::from_str(bytes).unwrap();
        malformed["principal"] = value;
        assert_eq!(
            Record::decode(&key, &principal, &malformed.to_string()).unwrap_err().code,
            ErrorCode::InvalidRequest,
        );
    }
    let mut missing: Value = json_codec::from_str(bytes).unwrap();
    missing.as_object_mut().unwrap().remove("principal");
    assert_eq!(
        Record::decode(&key, &principal, &missing.to_string()).unwrap_err().code,
        ErrorCode::InvalidRequest,
    );
    for length in [1024, 1025] {
        let mut bounded = saved.clone();
        bounded.principal = "a".repeat(length).parse().unwrap();
        let wire = json_codec::to_string(&bounded).unwrap();
        assert_eq!(
            Record::decode(&bounded.key(), &bounded.principal, &wire).is_ok(),
            length == 1024,
        );
    }
}
