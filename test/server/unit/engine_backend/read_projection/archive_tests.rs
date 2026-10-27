use super::*;
#[test]
fn typed_archive_preserves_raw_receipt_digest_and_resource_before_replay_order() {
    let (root, mut engine, repo, _) = fixture();
    let source = engine.state().histories[&repo.history_tree_id]
        .root_commit_id
        .clone();
    let branch = engine
        .create_branch(
            &source,
            &storage_contract::BranchName::parse("side").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let generation = branch.generation;
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let operation = storage_contract::OperationId::generate();
    let request = |branch: &str, operation_id| crate::ArchiveBranch {
        branch_id: branch.into(),
        operation_id,
    };
    let result = backend
        .archive_branch(&identity, request(&branch.branch_id, operation))
        .unwrap();
    assert_eq!(result.kind, "branch.archive");
    assert_eq!(
        backend
            .archive_branch(&identity, request(&branch.branch_id, operation))
            .unwrap(),
        result
    );
    assert_eq!(
        backend
            .engine
            .lock()
            .unwrap()
            .archive_branch(&branch.branch_id, &identity.principal, operation)
            .unwrap(),
        result
    );
    let expected=storage_contract::object_hash("request",&json!({"kind":"branch.archive","principal":identity.principal,"body":{"branch_id":branch.branch_id}})).unwrap();
    assert_eq!(result.request_digest, expected);
    let state = backend.engine.lock().unwrap();
    assert_eq!(
        state.state().branches[&branch.branch_id].generation,
        generation + 1
    );
    assert_eq!(
        state.state().branches[&branch.branch_id].state,
        storage_contract::BranchState::Archived
    );
    drop(state);
    assert_eq!(
        backend
            .archive_branch(&identity, request(&repo.main_branch_id, operation))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        backend
            .archive_branch(
                &identity,
                request(
                    &repo.main_branch_id,
                    storage_contract::OperationId::generate()
                )
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::MainBranchRequired
    );
    let missing = backend
        .archive_branch(&identity, request("missing", operation))
        .unwrap_err();
    assert_eq!(missing.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(missing.message, "branch not found");
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .archive_branch(&identity, request("missing", operation))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}
