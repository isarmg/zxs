//! Admission and stale-owner cases.
use super::*;

fn operation() -> OperationId {
    format!("op_{}", "a".repeat(32)).parse().unwrap()
}
fn receipt() -> Value {
    json!({
        "operation_id": operation(), "operation_kind": "file.nodes.retire",
        "request_digest": "b".repeat(64), "state": "completed", "commit_state": "PUBLISHED",
        "protection_at_publish": "NOT_APPLICABLE", "publication_sequence": "5", "error": null,
        "result": {"retirement": {"request": {
            "origin_file_id": format!("file_{}", "c".repeat(32)),
            "history_tree_id": format!("history_{}", "d".repeat(32)),
            "history_root": "e".repeat(64),
            "version_ids": [format!("ver_{}", "f".repeat(32))],
        }}},
    })
}
fn candidate() -> Candidate {
    Candidate::read((7, "admin".parse().unwrap()), operation(), receipt()).unwrap()
}
fn state() -> State {
    let mut state = State {
        authenticated: true,
        principal: Some("admin".parse().unwrap()),
        session_epoch: 7,
        ..Default::default()
    };
    state.management.storage = crate::management::Storage::Ready;
    state
}

#[test]
fn published_receipt_recovers_only_the_original_typed_selection_without_cleanup() {
    let candidate = candidate();
    assert_eq!(candidate.record.operation, operation());
    assert_eq!(candidate.record.format, Format::ResumeOnly);
    assert_eq!(candidate.record.label, candidate.record.file_id.to_string());
    assert_eq!(
        candidate.history.as_str(),
        format!("history_{}", "d".repeat(32))
    );
    assert!(matches_request(
        &candidate.record.file_id,
        candidate.record.history_root,
        &candidate.record.version_ids,
        &candidate.receipt
    ));
    let restored = candidate.record.restored_attempt(7).unwrap();
    assert!(restored.resume_only && restored.uncertain);
    assert!(restored.cleanup.is_none() && restored.separation.is_none());
    assert!(can_accept(&state(), &candidate));
}

#[test]
fn missing_incomplete_other_kind_or_other_operation_never_becomes_a_saved_request() {
    for (field, value) in [
        ("operation_id", json!(format!("op_{}", "0".repeat(32)))),
        ("operation_kind", json!("workspace.commit")),
        ("state", json!("in_doubt")),
        ("commit_state", json!("PREPARED")),
        ("request_digest", json!("not-a-digest")),
        ("result", Value::Null),
    ] {
        let mut response = receipt();
        response[field] = value;
        assert!(Candidate::read((7, "admin".parse().unwrap()), operation(), response).is_err());
    }
    assert!(Candidate::read((7, "admin".parse().unwrap()), operation(), Value::Null).is_err());
}

#[test]
fn original_request_is_strict_sorted_bounded_and_does_not_guess_a_current_path() {
    for (field, value) in [
        ("history_tree_id", json!("unknown")),
        ("history_root", json!("unknown")),
        ("origin_file_id", json!("unknown")),
        ("version_ids", json!([])),
        (
            "version_ids",
            json!([
                format!("ver_{}", "f".repeat(32)),
                format!("ver_{}", "f".repeat(32))
            ]),
        ),
        (
            "version_ids",
            json!([
                format!("ver_{}", "f".repeat(32)),
                format!("ver_{}", "0".repeat(32))
            ]),
        ),
        ("current_path", json!("invented/target")),
    ] {
        let mut response = receipt();
        response["result"]["retirement"]["request"][field] = value;
        assert!(Candidate::read((7, "admin".parse().unwrap()), operation(), response).is_err());
    }
    let mut response = receipt();
    let request = &response["result"]["retirement"]["request"];
    response["result"]["retirement"]["request"] = json!([
        request["history_tree_id"],
        request["history_root"],
        request["origin_file_id"],
        request["version_ids"],
    ]);
    assert!(Candidate::read((7, "admin".parse().unwrap()), operation(), response).is_err());
    let mut response = receipt();
    response["result"]["retirement"]["request"]["version_ids"] = json!(
        (0..=storage_contract::shared_history::MAX_SHARED_HISTORY_NODES)
            .map(|n| format!("ver_{n:032x}"))
            .collect::<Vec<_>>()
    );
    assert!(Candidate::read((7, "admin".parse().unwrap()), operation(), response).is_err());
}

#[test]
fn resume_only_restriction_survives_storage_without_becoming_publication_evidence() {
    let candidate = candidate();
    let bytes = candidate.record.encode().unwrap();
    let saved = Record::decode(&candidate.record.key(), &"admin".parse().unwrap(), &bytes).unwrap();
    let mut attempt = saved.restored_attempt(7).unwrap();
    attempt.retry_confirmed = true;
    let mut state = state();
    assert!(!attempt.retry_allowed(&state));
    assert!(!attempt.dismiss_allowed(&state));
    assert_eq!(Record::from_attempt(&attempt), saved);
    state.files.operations.push(FileOperation::from_receipt(
        &operation().to_string(),
        &receipt(),
    ));
    assert!(attempt.retry_allowed(&state));
    state.session_epoch = 8;
    assert!(!attempt.retry_allowed(&state));
    state.session_epoch = 7;
    state.files.operations = vec![FileOperation::rejected(&operation().to_string())];
    assert!(!attempt.dismiss_allowed(&state));
    assert!(attempt.cleanup.is_none());
}

#[test]
fn cancelled_edited_or_previous_session_controls_cannot_save_another_candidate() {
    Owner::new().with(|| {
        let candidate = candidate();
        let owner = candidate.owner.clone();
        let mut model = Lookup {
            owner: Some(owner.clone()),
            generation: 4,
            candidate: Some(candidate.clone()),
            ..Default::default()
        };
        assert!(model.current(&owner, 4));
        model.edit("another operation".into());
        assert!(!model.current(&owner, 4) && model.candidate.is_none());
        model.candidate = Some(candidate);
        let mut snapshot = state();
        snapshot.session_epoch = 8;
        let state = RwSignal::new(snapshot);
        let model = RwSignal::new(model);
        // Both return before storage or network: stale generation, then owner.
        accept(state, model, 4);
        accept(state, model, 5);
        assert!(model.with_untracked(|model| model.saved.is_none()));
        assert!(state.with_untracked(|s| s.files.node_retirement.attempt.is_none()));
    });
}

#[test]
fn retained_request_identity_ignores_only_label_and_existing_recovery_mode() {
    let recovered = candidate().record;
    let mut original = recovered.clone();
    original.label = "original-name.txt".into();
    original.format = Format::Request;
    assert!(original.same_request(&recovered));
    original.history_root = HistoryRoot::parse(&"0".repeat(64)).unwrap();
    assert!(!original.same_request(&recovered));
    let mut snapshot = state();
    snapshot
        .files
        .operations
        .push(FileOperation::rejected(&operation().to_string()));
    assert!(!can_accept(&snapshot, &candidate()));
}
