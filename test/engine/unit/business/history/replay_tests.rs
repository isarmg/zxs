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
        .create_repository("Branch replay", "P01", 1, "admin", &new_id("op"))
        .unwrap();
    let source = engine.state().histories[&repo.history_tree_id]
        .root_commit_id
        .clone();
    let operation = new_id("op");
    let original = engine
        .create_branch(&source, "side", "admin", &operation)
        .unwrap();
    let workspace = engine
        .create_workspace(&original.branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"later body")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "Later branch write".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .archive_branch(&original.branch_id, "admin", &new_id("op"))
        .unwrap();
    assert_ne!(engine.state().branches[&original.branch_id], original);
    for reopened in [false, true] {
        if reopened {
            drop(engine);
            engine = Engine::open(&root.path().join("system")).unwrap();
        }
        let before = canonical_json(engine.state()).unwrap();
        assert_eq!(
            engine
                .create_branch(&source, "side", "admin", &operation)
                .unwrap(),
            original
        );
        for (name, principal) in [("different", "admin"), ("side", "other")] {
            assert_eq!(
                engine
                    .create_branch(&source, name, principal, &operation)
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
            .create_repository("Replay", "P01", 3, "admin", &new_id("op"))
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
                operation_id: new_id("op"),
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
                "side",
                "admin",
                &new_id("op"),
            )
            .unwrap();
        let operation = new_id("op");
        let original = engine
            .add_merge_link(
                &side.branch_id,
                &side.head_commit_id,
                &repo.main_branch_id,
                &cut,
                "admin",
                &operation,
            )
            .unwrap();
        let plan = engine
            .create_split_plan(&repo.history_tree_id, &cut, "admin")
            .unwrap();
        engine
            .commit_plan(&plan.plan_id, "admin", &new_id("op"))
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
            let replay = engine
                .add_merge_link(
                    &side.branch_id,
                    &side.head_commit_id,
                    &repo.main_branch_id,
                    &cut,
                    "admin",
                    &operation,
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
                            &operation,
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
