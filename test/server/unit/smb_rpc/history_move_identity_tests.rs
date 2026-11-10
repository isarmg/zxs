//! Real backend regressions for original SMB move identity. These deliberately
//! bypass scripted replies and reuse the existing temporary-root fixture.
use super::*;
use storage_contract::history_window::HistoryFileId;

fn lookup(roots: &Roots, repository: &Repository, path: &str) -> Option<Entry> {
    roots
        .shared
        .backend
        .lookup_current(
            &identity(&roots.shared),
            &repository.main_branch_id,
            &name_policy::LookupPath::parse(path).unwrap(),
        )
        .unwrap()
}

fn reopen_backend(fixture: &Fixture) -> EngineBackend {
    let backend = EngineBackend::new(Engine::open(&fixture.path().join("state")).unwrap());
    backend
        .set_management_auth(crate::lock(&fixture.auth).reader())
        .unwrap();
    backend
}

fn retain_file_intent(
    roots: &Roots,
    operation: OperationId,
) -> (storage_contract::State, storage_engine::FileHistoryMoveRecoveryInfo) {
    let file = HistoryFileId::parse(&original_entry(roots).file_id).unwrap();
    let cancellation = storage_contract::history_window::HistoryWindowCancellation::new();
    // Use the existing real retained-material APIs. Fresh public move methods
    // may converge on object.move without changing an already selected intent.
    roots.backend.inner.test_with_engine_locked(|engine| {
        let before = engine.state().clone();
        let namespace = storage_engine::FileHistoryCopyRequest {
            source_repository_id: roots.source.repository_id.clone(),
            source_path: "source.txt".into(),
            expected_source_head: before.branches[&roots.source.main_branch_id]
                .head_commit_id
                .clone(),
            target_repository_id: roots.target.repository_id.clone(),
            target_name: "arrived.txt".into(),
            expected_target_head: before.branches[&roots.target.main_branch_id]
                .head_commit_id
                .clone(),
        };
        let capture = engine
            .capture_file_history_move(
                storage_engine::FileHistoryMoveRequest {
                    namespace: &namespace,
                    source_file_id: &file,
                },
                &cancellation,
            )
            .unwrap();
        let projection = engine
            .project_file_history_move(capture, "alice", operation, &cancellation)
            .unwrap();
        let intent = engine
            .save_file_history_move_recovery(&projection, &cancellation)
            .unwrap();
        assert_eq!(engine.state(), &before);
        assert!(!engine.state().operations.contains_key(&operation));
        (before, intent)
    })
}

#[test]
fn published_move_retry_does_not_capture_a_new_file_at_the_old_source_path() {
    let roots = roots();
    let operation = OperationId::generate().unwrap();
    let first = command(&roots, operation);
    let retry = command(&roots, operation);
    let old = original_entry(&roots);
    let receipt = roots
        .backend
        .inner
        .move_current_file(&identity(&roots.shared), first)
        .unwrap();
    assert!(lookup(&roots, &roots.source, "source.txt").is_none());
    let moved = lookup(&roots, &roots.target, "arrived.txt").unwrap();

    roots.backend.inner.test_with_engine_locked(|engine| {
        let workspace = engine
            .create_workspace(&roots.source.main_branch_id, "alice")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "source.txt", b"new owner of this path")
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "alice".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "reuse source after completed move".into(),
                request_digest: None,
            })
            .unwrap();
    });
    let replacement = original_entry(&roots);
    assert_ne!(replacement.entry_id, old.entry_id);
    assert_ne!(replacement.file_id, old.file_id);
    let before = roots
        .backend
        .inner
        .test_with_engine_locked(|engine| engine.state().clone());
    assert_eq!(
        roots
            .backend
            .inner
            .move_current_file(&identity(&roots.shared), retry)
            .unwrap(),
        receipt
    );
    roots
        .backend
        .inner
        .test_with_engine_locked(|engine| assert_eq!(engine.state(), &before));
    assert_eq!(original_entry(&roots), replacement);
    assert_eq!(lookup(&roots, &roots.target, "arrived.txt").unwrap(), moved);
}

#[test]
fn published_move_rejects_changed_original_source_entry_or_file_identity() {
    let roots = roots();
    let operation = OperationId::generate().unwrap();
    let first = command(&roots, operation);
    let mut changed_entry = command(&roots, operation);
    let mut changed_file = command(&roots, operation);
    changed_entry.expected_source_entry_id = new_id("entry").unwrap();
    changed_file.expected_source_file_id = HistoryFileId::parse(&new_id("file").unwrap()).unwrap();
    roots
        .backend
        .inner
        .move_current_file(&identity(&roots.shared), first)
        .unwrap();
    let before = roots
        .backend
        .inner
        .test_with_engine_locked(|engine| engine.state().clone());
    for changed in [changed_entry, changed_file] {
        let error = roots
            .backend
            .inner
            .move_current_file(&identity(&roots.shared), changed)
            .unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::IdempotencyKeyReused);
    }
    roots
        .backend
        .inner
        .test_with_engine_locked(|engine| assert_eq!(engine.state(), &before));
}

#[test]
fn replacement_retry_keeps_both_original_target_identities_after_publication() {
    let roots = roots_with_target(true);
    let operation = OperationId::generate().unwrap();
    let target = lookup(&roots, &roots.target, "arrived.txt").unwrap();
    let captured = || crate::ReplaceCurrentFile {
        request: command(&roots, operation),
        expected_target_entry_id: target.entry_id.clone(),
        expected_target_file_id: HistoryFileId::parse(&target.file_id).unwrap(),
    };
    let first = captured();
    let retry = captured();
    let mut changed_entry = captured();
    let mut changed_file = captured();
    changed_entry.expected_target_entry_id = new_id("entry").unwrap();
    changed_file.expected_target_file_id = HistoryFileId::parse(&new_id("file").unwrap()).unwrap();
    let receipt = roots
        .backend
        .inner
        .replace_current_file(&identity(&roots.shared), first)
        .unwrap();
    let before = roots
        .backend
        .inner
        .test_with_engine_locked(|engine| engine.state().clone());
    assert_eq!(
        roots
            .backend
            .inner
            .replace_current_file(&identity(&roots.shared), retry)
            .unwrap(),
        receipt
    );
    for changed in [changed_entry, changed_file] {
        let error = roots
            .backend
            .inner
            .replace_current_file(&identity(&roots.shared), changed)
            .unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::IdempotencyKeyReused);
    }
    roots
        .backend
        .inner
        .test_with_engine_locked(|engine| assert_eq!(engine.state(), &before));
    let moved = lookup(&roots, &roots.target, "arrived.txt").unwrap();
    assert_ne!(moved.entry_id, target.entry_id);
    assert_ne!(moved.file_id, target.file_id);
}

#[test]
fn retained_file_intent_retries_original_identity_without_a_second_operation() {
    let roots = roots();
    let operation = OperationId::generate().unwrap();
    let retry = command(&roots, operation);
    let mut wrong_entry = command(&roots, operation);
    let mut wrong_file = command(&roots, operation);
    wrong_entry.expected_source_entry_id = new_id("entry").unwrap();
    wrong_file.expected_source_file_id = HistoryFileId::parse(&new_id("file").unwrap()).unwrap();
    let old = original_entry(&roots);
    let (before, intent) = retain_file_intent(&roots, operation);
    for changed in [wrong_entry, wrong_file] {
        let error = roots
            .backend
            .inner
            .move_current_file(&identity(&roots.shared), changed)
            .unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::IdempotencyKeyReused);
        roots.backend.inner.test_with_engine_locked(|engine| {
            assert_eq!(engine.state(), &before);
            assert_eq!(
                engine.inspect_file_history_move_recovery().unwrap().as_ref(),
                Some(&intent)
            );
        });
    }
    assert_eq!(original_entry(&roots), old);
    assert!(lookup(&roots, &roots.target, "arrived.txt").is_none());
    let receipt = roots
        .backend
        .inner
        .move_current_file(&identity(&roots.shared), retry)
        .unwrap();
    assert_eq!(receipt.operation_id, operation);
    assert_eq!(receipt.kind, "file.history.move");
    assert_eq!(receipt.sequence, before.sequence + 1);
    assert_eq!(receipt.commit_state, CommitState::Published);
    roots.backend.inner.test_with_engine_locked(|engine| {
        assert_eq!(
            engine.file_deletion(&old.file_id).unwrap().operation_id,
            operation
        );
        assert_eq!(engine.state().operations[&operation], receipt);
        assert_eq!(
            engine
                .state()
                .events
                .iter()
                .filter(|event| event.operation_id == operation)
                .count(),
            1
        );
        assert!(engine.inspect_file_history_move_recovery().unwrap().is_none());
    });
    assert_eq!(intent.operation_id(), operation);
    assert!(lookup(&roots, &roots.source, "source.txt").is_none());
}

#[test]
fn canonical_source_alias_is_preserved_for_retry_after_the_source_disappears() {
    let roots = roots();
    let operation = OperationId::generate().unwrap();
    let mut first = command(&roots, operation);
    let mut retry = command(&roots, operation);
    for command in [&mut first, &mut retry] {
        command.source_path = name_policy::LookupPath::parse("e\u{301}.txt").unwrap();
    }
    roots.backend.inner.test_with_engine_locked(|engine| {
        engine
            .file_namespace(
                &roots.source.repository_id,
                storage_contract::NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Rename,
                    "source.txt",
                    Some("é.txt"),
                )
                .unwrap(),
                "alice",
                OperationId::generate().unwrap(),
            )
            .unwrap();
    });
    let receipt = roots
        .backend
        .inner
        .move_current_file(&identity(&roots.shared), first)
        .unwrap();
    assert!(lookup(&roots, &roots.source, "e\u{301}.txt").is_none());
    let before = roots
        .backend
        .inner
        .test_with_engine_locked(|engine| engine.state().clone());
    assert_eq!(
        roots
            .backend
            .inner
            .move_current_file(&identity(&roots.shared), retry)
            .unwrap(),
        receipt
    );
    roots
        .backend
        .inner
        .test_with_engine_locked(|engine| assert_eq!(engine.state(), &before));
}

#[test]
fn published_object_replay_rejects_revoked_and_reenabled_stale_smb_bindings() {
    for target in [false, true] {
        let roots = roots();
        let operation = OperationId::generate().unwrap();
        let first = object_command(&roots, operation);
        let retry = object_command(&roots, operation);
        let stale_after_reenable = object_command(&roots, operation);
        let receipt = roots
            .backend
            .inner
            .move_current_object(&identity(&roots.shared), first)
            .unwrap();
        let repository = if target { &roots.target } else { &roots.source };
        let binding = roots
            .backend
            .inner
            .smb_binding_status(&identity(&roots.shared), &repository.repository_id)
            .unwrap();
        roots
            .backend
            .inner
            .update_smb_binding(
                &identity(&roots.shared),
                crate::UpdateSmbBinding {
                    repository: &repository.repository_id,
                    branch: &repository.main_branch_id,
                    enabled: false,
                    expected_history_generation: binding.history_generation,
                    operation: OperationId::generate().unwrap(),
                },
            )
            .unwrap();
        let before = roots
            .backend
            .inner
            .test_with_engine_locked(|engine| engine.state().clone());
        let error = roots
            .backend
            .inner
            .move_current_object(&identity(&roots.shared), retry)
            .unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::SmbRebindRequired);
        roots.backend.inner.test_with_engine_locked(|engine| {
            assert_eq!(engine.state(), &before);
            assert_eq!(engine.state().operations.get(&operation), Some(&receipt));
        });

        // Enabling the same namespace again grants a new binding generation,
        // not permission for an old captured handle to bypass admission.
        let binding = roots
            .backend
            .inner
            .smb_binding_status(&identity(&roots.shared), &repository.repository_id)
            .unwrap();
        roots
            .backend
            .inner
            .update_smb_binding(
                &identity(&roots.shared),
                crate::UpdateSmbBinding {
                    repository: &repository.repository_id,
                    branch: &repository.main_branch_id,
                    enabled: true,
                    expected_history_generation: binding.history_generation,
                    operation: OperationId::generate().unwrap(),
                },
            )
            .unwrap();
        let reenabled = roots
            .backend
            .inner
            .test_with_engine_locked(|engine| engine.state().clone());
        assert_eq!(
            roots
                .backend
                .inner
                .move_current_object(&identity(&roots.shared), stale_after_reenable)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::SmbRebindRequired
        );
        roots.backend.inner.test_with_engine_locked(|engine| {
            assert_eq!(engine.state(), &reenabled);
            assert_eq!(engine.state().operations.get(&operation), Some(&receipt));
        });
    }
}

#[test]
fn cold_reopened_file_intent_finishes_through_smb_and_replays_its_original_receipt() {
    let roots = roots();
    let actor = identity(&roots.shared);
    let operation = OperationId::generate().unwrap();
    let retry = command(&roots, operation);
    let replay = command(&roots, operation);
    let old = original_entry(&roots);
    let (before, intent) = retain_file_intent(&roots, operation);
    let Roots {
        _fixture,
        shared,
        backend,
        source,
        target,
    } = roots;
    drop(shared);
    drop(backend);
    let backend = reopen_backend(&_fixture);
    backend.test_with_engine_locked(|engine| {
        assert!(!engine.state().operations.contains_key(&operation));
        assert_eq!(
            engine.inspect_file_history_move_recovery().unwrap().as_ref(),
            Some(&intent)
        );
    });
    let receipt = backend.move_current_file(&actor, retry).unwrap();
    assert_eq!(receipt.kind, "file.history.move");
    assert_eq!(receipt.sequence, before.sequence + 1);
    backend.test_with_engine_locked(|engine| {
        assert!(
            engine
                .namespace_tree(&source.main_branch_id)
                .unwrap()
                .get("source.txt")
                .is_none()
        );
        assert!(
            engine
                .namespace_tree(&target.main_branch_id)
                .unwrap()
                .get("arrived.txt")
                .is_some()
        );
        assert_eq!(
            engine.file_deletion(&old.file_id).unwrap().operation_id,
            operation
        );
    });
    drop(backend);
    let backend = reopen_backend(&_fixture);
    let published = backend.test_with_engine_locked(|engine| engine.state().clone());
    assert_eq!(backend.move_current_file(&actor, replay).unwrap(), receipt);
    backend.test_with_engine_locked(|engine| {
        assert_eq!(engine.state(), &published);
        assert_eq!(
            engine
                .state()
                .events
                .iter()
                .filter(|event| event.operation_id == operation)
                .count(),
            1
        );
        assert!(engine.inspect_file_history_move_recovery().unwrap().is_none());
    });
}

#[path = "history_move_completion_tests.rs"]
mod completion_tests;

#[test]
fn object_move_replay_rechecks_captured_disk_identity_and_generation() {
    let roots = roots();
    let actor = identity(&roots.shared);
    let operation = OperationId::generate().unwrap();
    let first = object_command(&roots, operation);
    let retry = object_command(&roots, operation);
    let mut wrong_source = object_command(&roots, operation);
    wrong_source.source_disk_id = new_id("disk").unwrap();
    let mut wrong_target = object_command(&roots, operation);
    let crate::MoveObjectTarget::Repository { binding_generation, .. } = &mut wrong_target.target
        else { panic!("fixture target must already have a root"); };
    *binding_generation += 1;
    let receipt = roots.backend.inner.move_current_object(&actor, first).unwrap();
    let published = roots.backend.inner.test_with_engine_locked(|engine| engine.state().clone());
    assert!(lookup(&roots, &roots.source, "source.txt").is_none());
    for command in [wrong_source, wrong_target] {
        assert_eq!(
            roots.backend.inner.move_current_object(&actor, command).unwrap_err().code,
            storage_contract::ErrorCode::WorkspaceChanged
        );
        roots.backend.inner.test_with_engine_locked(|engine| assert_eq!(engine.state(), &published));
    }
    assert_eq!(roots.backend.inner.move_current_object(&actor, retry).unwrap(), receipt);
    roots.backend.inner.test_with_engine_locked(|engine| assert_eq!(engine.state(), &published));
}
