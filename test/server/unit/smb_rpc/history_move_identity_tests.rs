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
fn retained_move_intent_retries_original_identity_without_a_second_operation() {
    let roots = roots();
    let operation = OperationId::generate().unwrap();
    let first = command(&roots, operation);
    let retry = command(&roots, operation);
    let old = original_entry(&roots);
    roots.backend.inner.test_with_engine_locked(|engine| {
        engine.inject_fault_once("move_after_original_intent");
    });
    assert!(
        roots
            .backend
            .inner
            .move_current_file(&identity(&roots.shared), first)
            .is_err()
    );
    let before = roots.backend.inner.test_with_engine_locked(|engine| {
        assert!(!engine.state().operations.contains_key(&operation));
        engine.inspect_file_history_move_recovery().unwrap().unwrap()
    });
    assert_eq!(original_entry(&roots), old);
    assert!(lookup(&roots, &roots.target, "arrived.txt").is_none());
    let receipt = roots
        .backend
        .inner
        .move_current_file(&identity(&roots.shared), retry)
        .unwrap();
    assert_eq!(receipt.operation_id, operation);
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
    assert_eq!(before.operation_id(), operation);
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
