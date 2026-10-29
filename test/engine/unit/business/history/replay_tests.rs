use super::*;

#[test]
fn branch_creation_replay_returns_original_record_after_writes_archive_and_reopen() {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("primary")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Branch replay").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let source_workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    for (path, bytes) in [
        ("file.txt", b"parent".as_slice()),
        ("folder/其他.txt", b"other".as_slice()),
    ] {
        engine
            .stage_bytes(&source_workspace.workspace_id, path, bytes)
            .unwrap();
    }
    let source = engine
        .commit_workspace(CommitRequest {
            workspace_id: source_workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: source_workspace.base_head,
            fencing_token: source_workspace.fencing_token,
            message: "Frozen parent sources".into(),
            request_digest: None,
        })
        .unwrap()
        .commit_id
        .unwrap();
    let operation = storage_contract::OperationId::generate();
    let original = engine
        .create_branch(
            &source,
            &storage_contract::BranchName::parse("side").unwrap(),
            "admin",
            operation,
        )
        .unwrap();
    assert_eq!(original.pending_file_versions.len(), 2);
    let original_bytes = canonical_json(&original).unwrap();
    let workspace = engine
        .create_workspace(&original.branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"later body")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "Later branch write".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .archive_branch(
            &original.branch_id,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_ne!(engine.state().branches[&original.branch_id], original);
    for reopened in [false, true] {
        if reopened {
            drop(engine);
            engine = Engine::open(&root.path().join("system")).unwrap();
        }
        let before = canonical_json(engine.state()).unwrap();
        assert_eq!(
            canonical_json(
                &engine.published_receipt(operation).unwrap().unwrap().result["created_branch"]
            )
            .unwrap(),
            original_bytes
        );
        assert_eq!(
            engine
                .create_branch(
                    &source,
                    &storage_contract::BranchName::parse("side").unwrap(),
                    "admin",
                    operation
                )
                .unwrap(),
            original
        );
        for (name, principal) in [("different", "admin"), ("side", "other")] {
            assert_eq!(
                engine
                    .create_branch(
                        &source,
                        &storage_contract::BranchName::parse(name).unwrap(),
                        principal,
                        operation
                    )
                    .unwrap_err()
                    .code(),
                storage_contract::ErrorCode::IdempotencyKeyReused
            );
        }
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
    }
}

#[test]
fn merge_link_retry_keeps_original_result_after_split_and_restart() {
    for crossing in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut engine = Engine::initialize(
            &root.path().join("system"),
            Config::development([
                root.path().join("p"),
                root.path().join("b"),
                root.path().join("c"),
            ]),
        )
        .unwrap();
        let repo = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Replay").unwrap(),
                "P01",
                3,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let root_commit = engine.state().histories[&repo.history_tree_id]
            .root_commit_id
            .clone();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file.txt", b"body")
            .unwrap();
        let cut = engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "cut".into(),
                request_digest: None,
            })
            .unwrap()
            .commit_id
            .unwrap();
        let side = engine
            .create_branch(
                if crossing { &root_commit } else { &cut },
                &storage_contract::BranchName::parse("side").unwrap(),
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let operation = storage_contract::OperationId::generate();
        let original = engine
            .add_merge_link(
                &side.branch_id,
                &side.head_commit_id,
                &repo.main_branch_id,
                &cut,
                "admin",
                operation,
            )
            .unwrap();
        assert_eq!(
            engine.state().merge_links[&original.merge_link_id],
            original
        );
        let receipt = engine.operation(operation).unwrap();
        assert_eq!(receipt.kind, "merge-link.create");
        assert_eq!(
            receipt.result,
            json!({
                "merge_link_id": original.merge_link_id,
                "history_tree_id": original.history_tree_id,
                "timestamp": receipt.result["timestamp"],
                "membership_epoch": "1",
                "version_protection": []
            })
        );
        let published = canonical_json(engine.state()).unwrap();
        assert_eq!(
            engine
                .add_merge_link(
                    &side.branch_id,
                    &side.head_commit_id,
                    &repo.main_branch_id,
                    &cut,
                    "admin",
                    operation,
                )
                .unwrap(),
            original
        );
        assert_eq!(canonical_json(engine.state()).unwrap(), published);
        let plan = engine
            .create_split_plan(&repo.history_tree_id, &cut, "admin")
            .unwrap();
        engine
            .commit_plan(
                &plan.plan_id,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        assert!(
            !engine
                .state()
                .merge_links
                .contains_key(&original.merge_link_id)
        );
        for reopened in [false, true] {
            if reopened {
                drop(engine);
                engine = Engine::open(&root.path().join("system")).unwrap();
            }
            let before = canonical_json(engine.state()).unwrap();
            assert_eq!(engine.operation(operation).unwrap().result, receipt.result);
            let replay = engine
                .add_merge_link(
                    &side.branch_id,
                    &side.head_commit_id,
                    &repo.main_branch_id,
                    &cut,
                    "admin",
                    operation,
                )
                .unwrap();
            assert_eq!(
                canonical_json(&replay).unwrap(),
                canonical_json(&original).unwrap()
            );
            for (target, actor) in [(&root_commit, "admin"), (&cut, "other")] {
                assert_eq!(
                    engine
                        .add_merge_link(
                            &side.branch_id,
                            &side.head_commit_id,
                            &repo.main_branch_id,
                            target,
                            actor,
                            operation,
                        )
                        .unwrap_err()
                        .code(),
                    storage_contract::ErrorCode::IdempotencyKeyReused
                );
            }
            assert_eq!(canonical_json(engine.state()).unwrap(), before);
        }
    }
}

#[test]
fn merge_link_creation_does_not_return_candidate_when_publication_fails() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("system");
    let mut engine = Engine::initialize(
        &root,
        Config::development([temporary.path().join("primary")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Interrupted link").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let source = engine.state().histories[&repo.history_tree_id]
        .root_commit_id
        .clone();
    let side = engine
        .create_branch(
            &source,
            &storage_contract::BranchName::parse("side").unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let operation = OperationId::generate();
    let before = canonical_json(engine.state()).unwrap();
    engine.inject_fault_once("after_prepare_1");
    assert_eq!(
        engine
            .add_merge_link(
                &side.branch_id,
                &side.head_commit_id,
                &repo.main_branch_id,
                &source,
                "admin",
                operation
            )
            .unwrap_err()
            .code(),
        ErrorCode::InjectedFailure
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    drop(engine);
    let mut engine = Engine::open(&root).unwrap();
    let created = engine
        .add_merge_link(
            &side.branch_id,
            &side.head_commit_id,
            &repo.main_branch_id,
            &source,
            "admin",
            operation,
        )
        .unwrap();
    assert_eq!(engine.state().merge_links[&created.merge_link_id], created);
    assert_eq!(
        engine
            .add_merge_link(
                &side.branch_id,
                &side.head_commit_id,
                &repo.main_branch_id,
                &source,
                "admin",
                operation
            )
            .unwrap(),
        created
    );
}
