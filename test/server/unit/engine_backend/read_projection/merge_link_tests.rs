use super::*;
#[test]
fn typed_merge_link_keeps_receipt_binding_and_scope_admission() {
    let (root, mut engine, repo, _) = fixture();
    let head = engine.state().histories[&repo.history_tree_id]
        .root_commit_id
        .clone();
    let side = engine
        .create_branch(
            &head,
            &storage_contract::BranchName::parse("side").unwrap(),
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let operation = storage_contract::OperationId::generate();
    let request =
        |history: &str, source: &str, target_commit: &str, operation_id| crate::CreateMergeLink {
            history_tree_id: history.into(),
            source_branch_id: source.into(),
            source_commit_id: side.head_commit_id.clone(),
            target_branch_id: repo.main_branch_id.clone(),
            target_commit_id: target_commit.into(),
            operation_id,
        };
    let result = backend
        .create_merge_link(
            &identity,
            request(&repo.history_tree_id, &side.branch_id, &head, operation),
        )
        .unwrap();
    let expected=storage_contract::object_hash("request",&json!({"kind":"merge-link.create","principal":identity.principal,"body":{"source_branch_id":side.branch_id,"source_commit_id":side.head_commit_id,"target_branch_id":repo.main_branch_id,"target_commit_id":head}})).unwrap();
    let mut engine = backend.engine.lock().unwrap();
    assert_eq!(
        engine
            .state()
            .operations
            .tail_record(operation)
            .unwrap()
            .result
            .request_digest,
        expected
    );
    assert_eq!(
        engine
            .add_merge_link(
                &side.branch_id,
                &side.head_commit_id,
                &repo.main_branch_id,
                &head,
                &identity.principal,
                operation
            )
            .unwrap(),
        result
    );
    drop(engine);
    assert_eq!(
        backend
            .create_merge_link(
                &identity,
                request(&repo.history_tree_id, "missing", "missing", operation)
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        backend
            .create_merge_link(
                &identity,
                request(
                    &repo.history_tree_id,
                    "missing",
                    &head,
                    storage_contract::OperationId::generate()
                )
            )
            .unwrap_err()
            .status,
        http::StatusCode::NOT_FOUND
    );
    assert_eq!(
        backend
            .create_merge_link(
                &identity,
                request("missing", &side.branch_id, &head, operation)
            )
            .unwrap_err()
            .status,
        http::StatusCode::NOT_FOUND
    );
    assert_eq!(
        backend
            .create_merge_link(
                &identity,
                request(&repo.history_tree_id, &side.branch_id, &head, operation)
            )
            .unwrap(),
        result
    );
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .create_merge_link(
                &identity,
                request("missing", "missing", "missing", operation)
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}
