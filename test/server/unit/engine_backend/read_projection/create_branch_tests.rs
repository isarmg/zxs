use super::*;
#[test]
fn typed_creation_binds_digest_and_checks_scope_before_receipt() {
    let (root, mut engine, repo, _) = fixture();
    let source = engine.state().histories[&repo.history_tree_id]
        .root_commit_id
        .clone();
    let other = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Other").unwrap(),
            "P01",
            3,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let outside = engine.state().histories[&other.history_tree_id]
        .root_commit_id
        .clone();
    let (backend, mut auth, token, identity) = authenticated_backend(engine, root.path());
    let operation = storage_contract::OperationId::generate();
    let request = |repository: &str, source: &str, name: &str, operation_id| crate::CreateBranch {
        repository_id: repository.into(),
        source_commit_id: source.into(),
        name: storage_contract::BranchName::parse(name).unwrap(),
        operation_id,
    };
    let created = backend
        .create_branch(
            &identity,
            request(&repo.repository_id, &source, "Café", operation),
        )
        .unwrap();
    let expected=storage_contract::object_hash("request",&json!({"kind":"branch.create","principal":identity.principal,"body":{"source_commit_id":source,"name":"Café"}})).unwrap();
    let mut engine = backend.engine.lock().unwrap();
    assert_eq!(
        engine.state().operations[&operation].request_digest,
        expected
    );
    let direct = engine
        .create_branch(
            &source,
            &storage_contract::BranchName::parse("Café").unwrap(),
            &identity.principal,
            operation,
        )
        .unwrap();
    assert_eq!(direct.branch_id, created.branch_id);
    drop(engine);
    assert_eq!(
        backend
            .create_branch(
                &identity,
                request(&repo.repository_id, &source, "Other", operation)
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        backend
            .create_branch(
                &identity,
                request(
                    &repo.repository_id,
                    &source,
                    "CAFÉ",
                    storage_contract::OperationId::generate()
                )
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::NameCollision
    );
    for source in [&outside, "missing"] {
        assert_eq!(
            backend
                .create_branch(
                    &identity,
                    request(&repo.repository_id, source, "Other", operation)
                )
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::HistoryNodeNotFound
        );
    }
    assert_eq!(
        backend
            .create_branch(&identity, request("missing", &source, "Other", operation))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::NotFound
    );
    backend
        .archive_branch(
            &identity,
            crate::ArchiveBranch {
                branch_id: created.branch_id.clone(),
                operation_id: storage_contract::OperationId::generate(),
            },
        )
        .unwrap();
    let replay = backend
        .create_branch(
            &identity,
            request(&repo.repository_id, &source, "Café", operation),
        )
        .unwrap();
    assert_eq!(replay.state, storage_contract::BranchState::Active);
    assert_eq!(replay.generation, 1);
    assert_eq!(replay.head_commit_id, created.head_commit_id);
    auth.logout(&token).unwrap();
    assert_eq!(
        backend
            .create_branch(&identity, request("missing", "missing", "Other", operation))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}
