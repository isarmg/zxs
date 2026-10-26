use super::*;

#[test]
fn disk_root_creation_replay_checks_owner_input_and_returns_original_binding() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("system");
    let mut config = Config::development([
        temporary.path().join("primary"),
        temporary.path().join("other-primary"),
    ]);
    config.disks[1].slot_id = "P02".into();
    config.disks[1].level = 1;
    config.disks[1].upstream_slot_id = None;
    let mut engine = Engine::initialize(&root, config).unwrap();
    let operation = storage_contract::OperationId::generate();
    let original = engine.create_disk_root("p01", "admin", operation).unwrap();
    let other = engine
        .create_disk_root("P02", "admin", storage_contract::OperationId::generate())
        .unwrap();
    let binding_operation = storage_contract::OperationId::generate();
    engine
        .set_smb_binding(
            &original.repository_id,
            &original.main_branch_id,
            false,
            1,
            "admin",
            binding_operation,
        )
        .unwrap();
    for reopened in [false, true] {
        if reopened {
            drop(engine);
            engine = Engine::open(&root).unwrap();
        }
        let before = canonical_json(engine.state()).unwrap();
        assert_eq!(
            engine.create_disk_root("P01", "admin", operation).unwrap(),
            original
        );
        for (slot, principal, operation) in [
            ("P01", "other", operation),
            ("P02", "admin", operation),
            ("P01", "admin", binding_operation),
        ] {
            assert_eq!(
                engine
                    .create_disk_root(slot, principal, operation)
                    .unwrap_err()
                    .code(),
                storage_contract::ErrorCode::IdempotencyKeyReused
            );
        }
        assert_eq!(
            "not-an-operation"
                .parse::<OperationId>()
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::InvalidId
        );
        assert_eq!(
            engine
                .create_disk_root("P01", "", storage_contract::OperationId::generate())
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::Unauthenticated
        );
        assert_eq!(
            engine
                .create_disk_root("P02", "admin", storage_contract::OperationId::generate())
                .unwrap(),
            other
        );
        let current = engine.state().repositories[&original.repository_id].clone();
        assert_ne!(current, original);
        assert_eq!(
            engine
                .create_disk_root("P01", "admin", storage_contract::OperationId::generate())
                .unwrap(),
            current
        );
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
    }
}

#[test]
fn repository_creation_replay_returns_original_binding_after_changes_and_reopen() {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("primary")]),
    )
    .unwrap();
    let operation = storage_contract::OperationId::generate();
    let original = engine
        .create_repository("Replay", "P01", 1, "admin", operation)
        .unwrap();
    let source = engine.state().histories[&original.history_tree_id]
        .root_commit_id
        .clone();
    let side = engine
        .create_branch(
            &source,
            "side",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
        .set_smb_binding(
            &original.repository_id,
            &side.branch_id,
            false,
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_ne!(
        engine.state().repositories[&original.repository_id],
        original
    );
    for reopened in [false, true] {
        if reopened {
            drop(engine);
            engine = Engine::open(&root.path().join("system")).unwrap();
        }
        let before = canonical_json(engine.state()).unwrap();
        assert_eq!(
            engine
                .create_repository("Replay", "P01", 1, "admin", operation)
                .unwrap(),
            original
        );
        for (name, principal) in [("Different", "admin"), ("Replay", "other")] {
            assert_eq!(
                engine
                    .create_repository(name, "P01", 1, principal, operation)
                    .unwrap_err()
                    .code(),
                storage_contract::ErrorCode::IdempotencyKeyReused
            );
        }
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
    }
}

#[test]
fn disk_root_uses_one_explicit_primary_on_short_and_complete_routes() {
    for disks in [1, 2, 3] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let mut config = Config::development([
            temporary.path().join("primary"),
            temporary.path().join("backup"),
            temporary.path().join("tertiary"),
        ]);
        config.disks.truncate(disks);
        let mut engine = Engine::initialize(&root, config).unwrap();
        let repo = engine
            .create_disk_root("p01", "admin", storage_contract::OperationId::generate())
            .unwrap();
        assert!(repo.file_manager_root);
        assert_eq!(repo.primary_slot_id, "P01");
        assert_eq!(repo.target_copies, 1);
        assert_eq!(
            engine
                .create_disk_root("P01", "admin", storage_contract::OperationId::generate())
                .unwrap()
                .repository_id,
            repo.repository_id
        );
        engine
            .file_namespace(
                &repo.repository_id,
                NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "Documents", None)
                    .unwrap(),
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(
                &workspace.workspace_id,
                "Documents/report.txt",
                b"one primary",
            )
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "Save on configured primary".into(),
                request_digest: None,
            })
            .unwrap();
        let entry = engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .into_iter()
            .find(|entry| entry.kind == NamespaceEntryKind::File)
            .unwrap();
        let placement = &engine.state().placements[&entry.version_id];
        assert_eq!(placement.target_copies, 1);
        assert_eq!(placement.route_slot_ids, vec!["P01".to_owned()]);
        drop(engine);
        let engine = Engine::open(&root).unwrap();
        let mut content = engine.open_content(&entry.version_id).unwrap();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut content.file, &mut bytes).unwrap();
        assert_eq!(bytes, b"one primary");
    }
}

#[test]
fn explicit_smb_binding_changes_advance_a_persistent_independent_revision() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("system");
    let mut engine = Engine::initialize(
        &root,
        Config::development([
            temporary.path().join("primary"),
            temporary.path().join("backup"),
            temporary.path().join("tertiary"),
        ]),
    )
    .unwrap();
    let repo = engine
        .create_disk_root("P01", "admin", storage_contract::OperationId::generate())
        .unwrap();
    assert_eq!(repo.smb_binding_generation, 1);
    for (index, enabled) in [true, false, true].into_iter().enumerate() {
        let expected = engine.state().histories[&repo.history_tree_id].generation;
        let operation = storage_contract::OperationId::generate();
        let first = engine
            .set_smb_binding(
                &repo.repository_id,
                &repo.main_branch_id,
                enabled,
                expected,
                "admin",
                operation,
            )
            .unwrap();
        assert_eq!(
            engine.state().repositories[&repo.repository_id].smb_binding_generation,
            index as u64 + 2
        );
        let replay = engine
            .set_smb_binding(
                &repo.repository_id,
                &repo.main_branch_id,
                enabled,
                expected,
                "admin",
                operation,
            )
            .unwrap();
        assert_eq!(first.result, replay.result);
        assert_eq!(
            engine.state().repositories[&repo.repository_id].smb_binding_generation,
            index as u64 + 2
        );
    }
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    assert_eq!(
        engine.state().repositories[&repo.repository_id].smb_binding_generation,
        4
    );
}
