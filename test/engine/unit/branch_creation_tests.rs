//! Fresh branch results and replay share the same immutable creation record.
use super::*;
use json_codec::json;

#[test]
fn fresh_branch_matches_complete_receipt_and_canonical_shape() {
    for populated in [false, true] {
        let fixture = Fixture::new();
        let mut engine = fixture.initialize();
        let repo = repository(&mut engine, 3);
        let source = if populated {
            save(&mut engine, &repo.main_branch_id, "first.txt", b"first");
            save(
                &mut engine,
                &repo.main_branch_id,
                "folder/其他.txt",
                b"second",
            )
            .commit_id
            .unwrap()
        } else {
            engine.state().histories[&repo.history_tree_id]
                .root_commit_id
                .clone()
        };
        let expected_pending = engine
            .namespace_entries(&engine.state().commits[&source].namespace)
            .unwrap()
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
            .map(|entry| (entry.file_id.clone(), entry.version_id.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let name = BranchName::parse("Café").unwrap();
        let operation = OperationId::generate();
        let created = engine
            .create_branch(&source, &name, "admin", operation)
            .unwrap();
        let receipt = engine.published_receipt(operation).unwrap().unwrap();
        let decoded: Branch =
            json_codec::from_value(receipt.result["created_branch"].clone()).unwrap();
        assert_eq!(created, decoded);
        assert_eq!(created.pending_file_versions, expected_pending);
        assert_eq!(created, engine.state().branches[&created.branch_id]);
        let mut expected = json!({
            "branch_id": created.branch_id,
            "history_tree_id": repo.history_tree_id,
            "name": "Café",
            "head_commit_id": created.head_commit_id,
            "fork_commit_id": source,
            "branch_kind": "full",
            "generation": "1",
            "state": "active"
        });
        if !expected_pending.is_empty() {
            expected["pending_file_versions"] = json!(expected_pending);
        }
        assert_eq!(
            canonical_json(&created).unwrap(),
            canonical_json(&expected).unwrap()
        );
        assert_eq!(
            canonical_json(&receipt.result["created_branch"]).unwrap(),
            canonical_json(&expected).unwrap()
        );
        let expected_result = json!({
            "branch_id": created.branch_id,
            "commit_id": created.head_commit_id,
            "history_tree_id": repo.history_tree_id,
            "created_branch": expected,
            "copied_files": "0",
            "placeholder_files": expected_pending.len().to_string(),
            "timestamp": receipt.result["timestamp"],
            "branch_generation": "1",
            "membership_epoch": "1",
            "version_protection": []
        });
        assert_eq!(
            canonical_json(&receipt.result).unwrap(),
            canonical_json(&expected_result).unwrap()
        );
        assert_eq!(
            receipt.request_digest,
            object_hash(
                "request",
                &json!({
                    "kind": "branch.create", "principal": "admin",
                    "body": {"source_commit_id": source, "name": "Café"}
                })
            )
            .unwrap()
        );
        drop(engine);
        let mut engine = Engine::open(&fixture.root).unwrap();
        let before = canonical_json(engine.state()).unwrap();
        assert_eq!(
            engine
                .create_branch(&source, &name, "admin", operation)
                .unwrap(),
            created
        );
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
        assert_eq!(
            canonical_json(&engine.published_receipt(operation).unwrap().unwrap().result).unwrap(),
            canonical_json(&expected_result).unwrap()
        );
    }
}

#[test]
fn branch_replay_rejects_damaged_receipt_with_json_source_not_live_fallback() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    let source = engine.state().histories[&repo.history_tree_id]
        .root_commit_id
        .clone();
    let name = BranchName::parse("side").unwrap();
    let operation = OperationId::generate();
    let created = engine
        .create_branch(&source, &name, "admin", operation)
        .unwrap();
    let original = engine.published_receipt(operation).unwrap().unwrap();
    for payload in [
        None,
        Some(json!("wrong type")),
        Some(json!({"branch_id": created.branch_id})),
    ] {
        let state = std::sync::Arc::make_mut(&mut engine.state);
        let mut records = state.operations.tail().to_vec();
        let receipt = records
            .iter_mut()
            .find(|entry| entry.result.operation_id == operation)
            .unwrap();
        receipt.result = original.clone();
        let fields = receipt.result.result.as_object_mut().unwrap();
        fields.remove("created_branch");
        if let Some(payload) = payload {
            fields.insert("created_branch".into(), payload);
        }
        state.operations = storage_contract::OperationLog::from_records(records).unwrap();
        let before = canonical_json(engine.state()).unwrap();
        let error = engine
            .create_branch(&source, &name, "admin", operation)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::StateInvalid);
        assert_eq!(error.message(), "created branch receipt is invalid");
        let Error::Context { source, .. } = error else {
            panic!("receipt failure must retain its cause")
        };
        let Error::Contract(storage_contract::Error::Json { source, .. }) = source.as_ref() else {
            panic!("receipt failure must retain its concrete JSON cause")
        };
        assert!(source.is_data());
        assert!(!source.to_string().is_empty());
        assert_eq!(engine.state().branches[&created.branch_id], created);
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
    }
}

#[test]
fn branch_creation_keeps_receipt_and_new_write_error_priority() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    let source = engine.state().histories[&repo.history_tree_id]
        .root_commit_id
        .clone();
    let name = BranchName::parse("side").unwrap();
    let operation = OperationId::generate();
    let original = engine
        .create_branch(&source, &name, "admin", operation)
        .unwrap();
    engine.blocked = true;
    let before = canonical_json(engine.state()).unwrap();
    assert_eq!(
        engine
            .create_branch(&source, &name, "admin", operation)
            .unwrap(),
        original
    );
    for (selected, spelling, actor, id, expected) in [
        (
            &source[..],
            "side",
            "",
            operation,
            ErrorCode::Unauthenticated,
        ),
        (
            &source[..],
            "side",
            "other",
            operation,
            ErrorCode::IdempotencyKeyReused,
        ),
        (
            &source[..],
            "changed",
            "admin",
            operation,
            ErrorCode::IdempotencyKeyReused,
        ),
        (
            "missing",
            "side",
            "admin",
            operation,
            ErrorCode::IdempotencyKeyReused,
        ),
        (
            &source[..],
            "side",
            "admin",
            OperationId::generate(),
            ErrorCode::RecoveryRequired,
        ),
        (
            "missing",
            "side",
            "admin",
            OperationId::generate(),
            ErrorCode::RecoveryRequired,
        ),
    ] {
        assert_eq!(
            engine
                .create_branch(selected, &BranchName::parse(spelling).unwrap(), actor, id)
                .unwrap_err()
                .code(),
            expected
        );
    }
    engine.blocked = false;
    assert_eq!(
        engine
            .create_branch("missing", &name, "admin", OperationId::generate())
            .unwrap_err()
            .code(),
        ErrorCode::CommitNotFound
    );
    assert_eq!(
        engine
            .create_branch(&source, &name, "admin", OperationId::generate())
            .unwrap_err()
            .code(),
        ErrorCode::NameCollision
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
}

#[cfg(feature = "fault-injection")]
#[test]
fn fresh_branch_is_not_returned_before_publication_and_recovery_replays_once() {
    for (point, accepted) in [
        ("after_payloads", false),
        ("after_prepare_1", false),
        ("after_accept_1", true),
        ("after_finalize", true),
        ("after_publication_1", true),
    ] {
        let fixture = Fixture::new();
        let mut engine = fixture.initialize();
        let repo = repository(&mut engine, 3);
        let source = save(&mut engine, &repo.main_branch_id, "parent.txt", b"frozen")
            .commit_id
            .unwrap();
        let name = BranchName::parse("side").unwrap();
        let operation = OperationId::generate();
        let branches = engine.state().branches.len();
        let operations = engine.state().operations.len();
        let before = canonical_json(engine.state()).unwrap();
        engine.inject_fault_once(point);
        let error = engine
            .create_branch(&source, &name, "admin", operation)
            .unwrap_err();
        assert_eq!(
            error.code(),
            if accepted {
                ErrorCode::InDoubt
            } else {
                ErrorCode::InjectedFailure
            },
            "{point}"
        );
        if accepted {
            let Error::Context { source, .. } = error else {
                panic!("indeterminate publication must retain its cause")
            };
            assert_eq!(source.code(), ErrorCode::InjectedFailure);
            assert!(engine.is_read_only());
        }
        assert_eq!(canonical_json(engine.state()).unwrap(), before, "{point}");
        drop(engine);
        let mut engine = Engine::open(&fixture.root).unwrap();
        assert_eq!(
            engine.published_receipt(operation).unwrap().is_some(),
            accepted,
            "{point}"
        );
        let recovered = canonical_json(engine.state()).unwrap();
        let created = engine
            .create_branch(&source, &name, "admin", operation)
            .unwrap();
        if accepted {
            assert_eq!(
                canonical_json(engine.state()).unwrap(),
                recovered,
                "{point}"
            );
        }
        assert_eq!(created.pending_file_versions.len(), 1);
        assert_eq!(engine.state().branches.len(), branches + 1, "{point}");
        assert_eq!(engine.state().operations.len(), operations + 1, "{point}");
        let receipt = engine.published_receipt(operation).unwrap().unwrap();
        assert_eq!(receipt.commit_state, CommitState::Published);
        assert_eq!(
            canonical_json(&created).unwrap(),
            canonical_json(&receipt.result["created_branch"]).unwrap()
        );
        let before = canonical_json(engine.state()).unwrap();
        assert_eq!(
            engine
                .create_branch(&source, &name, "admin", operation)
                .unwrap(),
            created
        );
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
    }
}
