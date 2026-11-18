use super::*;

#[test]
fn disk_root_creation_replay_checks_owner_input_and_returns_original_binding() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("system");
    let mut config = Config::development(crate::test_disks(
        [
            temporary.path().join("primary"),
            temporary.path().join("other-primary"),
        ],
    ))
    .unwrap();
    config.disks[1].slot_id = "P02".into();
    config.disks[1].level = 1;
    config.disks[1].upstream_slot_id = None;
    let mut engine = Engine::initialize(&root, config).unwrap();
    let operation = storage_contract::OperationId::generate().unwrap();
    // A fresh genesis binding is not permission to adopt unexplained contents
    // as the empty file-manager namespace. No metadata decision is made.
    let common = engine.config().disks[0].root.join("common-data");
    let occupied = common.join("unregistered-user-directory");
    std::fs::create_dir(&occupied).unwrap();
    let before = canonical_json(engine.state()).unwrap();
    assert_eq!(
        engine.create_disk_root("p01", "admin", operation).unwrap_err().code(),
        ErrorCode::PublicationInvalid,
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert!(occupied.is_dir());
    std::fs::remove_dir(&occupied).unwrap();
    let original = engine.create_disk_root("p01", "admin", operation).unwrap();
    let original_id = original.repository_id.parse().unwrap();
    let original_main = original.main_branch_id.parse().unwrap();
    let origin = engine.operation(operation).unwrap().result["file_manager_root_origin"].clone();
    let binding = engine.config().storage_roots[
        &engine.config().disks[0].disk_id.parse::<storage_contract::DiskId>().unwrap()
    ].clone();
    {
        let source = engine.namespace_host_root(original_id, original_main).unwrap();
        assert_eq!(source.binding(), &binding);
        assert_eq!(source.common().unwrap().path(), common);
        assert_eq!(source.disk().disk_id, binding.registration.disk_id);
    }
    let other = engine
        .create_disk_root(
            "P02",
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let binding_operation = storage_contract::OperationId::generate().unwrap();
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
        let source = engine.namespace_host_root(original_id, original_main).unwrap();
        assert_eq!(source.binding(), &binding);
        assert_eq!(source.common().unwrap().path(), common);
        assert_eq!(engine.operation(operation).unwrap().result["file_manager_root_origin"], origin);
        drop(source);
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
                .create_disk_root(
                    "P01",
                    "",
                    storage_contract::OperationId::generate().unwrap()
                )
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::Unauthenticated
        );
        assert_eq!(
            engine
                .create_disk_root(
                    "P02",
                    "admin",
                    storage_contract::OperationId::generate().unwrap()
                )
                .unwrap(),
            other
        );
        let current = engine.state().repositories[&original.repository_id].clone();
        assert_ne!(current, original);
        assert_eq!(
            engine
                .create_disk_root(
                    "P01",
                    "admin",
                    storage_contract::OperationId::generate().unwrap()
                )
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
        Config::development(crate::test_disks([root.path().join("primary")])).unwrap(),
    )
    .unwrap();
    let operation = storage_contract::OperationId::generate().unwrap();
    let original = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Replay").unwrap(),
            "P01",
            1,
            "admin",
            operation,
        )
        .unwrap();
    // Ordinary repositories have no implicit physical host, even when their
    // primary's genesis common root exists and their namespace is empty.
    assert_eq!(
        engine.namespace_host_root(
            original.repository_id.parse().unwrap(), original.main_branch_id.parse().unwrap(),
        ).err().unwrap().code(),
        ErrorCode::UnsupportedOperation,
    );
    let source = engine.state().histories[&original.history_tree_id]
        .root_commit_id
        .clone();
    let side = engine
        .create_branch(
            &source,
            &storage_contract::BranchName::parse("side").unwrap(),
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let old_state = engine.state().clone();
    assert_eq!(
        main_for_branch(&old_state, &original.main_branch_id),
        Some(&original)
    );
    assert!(main_for_branch(&old_state, &side.branch_id).is_none());
    assert!(main_for_branch(&old_state, "missing-branch").is_none());
    engine
        .set_smb_binding(
            &original.repository_id,
            &side.branch_id,
            false,
            1,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
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
        assert!(main_for_branch(engine.state(), &original.main_branch_id).is_none());
        assert_eq!(
            main_for_branch(engine.state(), &side.branch_id),
            engine.state().repositories.get(&original.repository_id)
        );
        assert_eq!(
            main_for_branch(&old_state, &original.main_branch_id),
            Some(&original)
        );
        assert!(main_for_branch(&old_state, &side.branch_id).is_none());
        let before = canonical_json(engine.state()).unwrap();
        assert_eq!(
            engine
                .create_repository(
                    &storage_contract::RepositoryName::parse("Replay").unwrap(),
                    "P01",
                    1,
                    "admin",
                    operation
                )
                .unwrap(),
            original
        );
        for (name, principal) in [("Different", "admin"), ("Replay", "other")] {
            assert_eq!(
                engine
                    .create_repository(
                        &storage_contract::RepositoryName::parse(name).unwrap(),
                        "P01",
                        1,
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
fn disk_root_uses_one_explicit_primary_on_short_and_complete_routes() {
    for disks in [1, 2, 3] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let mut config = Config::development(crate::test_disks(
            [
                temporary.path().join("primary"),
                temporary.path().join("backup"),
                temporary.path().join("tertiary"),
            ],
        ))
        .unwrap();
        config.disks.truncate(disks);
        let mut engine = Engine::initialize(&root, config).unwrap();
        let repo = engine
            .create_disk_root(
                "p01",
                "admin",
                storage_contract::OperationId::generate().unwrap(),
            )
            .unwrap();
        assert!(repo.file_manager_root);
        assert_eq!(repo.primary_slot_id, "P01");
        assert_eq!(repo.target_copies, 1);
        assert_eq!(
            engine
                .create_disk_root(
                    "P01",
                    "admin",
                    storage_contract::OperationId::generate().unwrap()
                )
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
                storage_contract::OperationId::generate().unwrap(),
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
                operation_id: storage_contract::OperationId::generate().unwrap(),
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
        Config::development(crate::test_disks(
            [
                temporary.path().join("primary"),
                temporary.path().join("backup"),
                temporary.path().join("tertiary"),
            ],
        ))
        .unwrap(),
    )
    .unwrap();
    let repo = engine
        .create_disk_root(
            "P01",
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(repo.smb_binding_generation, 1);
    for (index, enabled) in [true, false, true].into_iter().enumerate() {
        let expected = engine.state().histories[&repo.history_tree_id].generation;
        let operation = storage_contract::OperationId::generate().unwrap();
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

#[test]
#[cfg(feature = "fault-injection")]
fn file_manager_root_authority_follows_actual_publication_and_cold_readback() {
    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let disk = temporary.path().join("primary");
    let mut engine = Engine::initialize(
        &system, Config::development(crate::test_disks([disk.clone()])).unwrap(),
    ).unwrap();
    let operation = OperationId::generate().unwrap();
    let before = canonical_json(engine.state()).unwrap();
    let roots = engine.config().storage_roots.clone();
    engine.inject_fault_once("after_local_publications");
    let error = engine.create_disk_root("P01", "admin", operation).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InDoubt);
    let Error::Context { source, .. } = error else {
        panic!("publication failure must retain the actual injected cause");
    };
    assert_eq!(source.code(), ErrorCode::InjectedFailure);
    assert!(source.message().contains("after_local_publications"));
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert!(!engine.state().operations.contains_key(&operation));
    assert!(engine.state().repositories.is_empty());
    assert_eq!(std::fs::read_dir(disk.join("common-data")).unwrap().count(), 0);
    drop(engine);

    let mut engine = Engine::open(&system).unwrap();
    let receipt = engine.operation(operation).unwrap();
    assert_eq!(receipt.commit_state, storage_contract::CommitState::Published);
    let repository = engine.create_disk_root("P01", "admin", operation).unwrap();
    let source = engine.namespace_host_root(
        repository.repository_id.parse().unwrap(), repository.main_branch_id.parse().unwrap(),
    ).unwrap();
    let disk_id = source.disk().disk_id.parse::<storage_contract::DiskId>().unwrap();
    assert_eq!(source.binding(), &roots[&disk_id]);
    assert_eq!(source.common().unwrap().path(), disk.join("common-data"));
    drop(source);
    let original = canonical_json(engine.state()).unwrap();
    assert_eq!(engine.create_disk_root("P01", "admin", operation).unwrap(), repository);
    assert_eq!(canonical_json(engine.state()).unwrap(), original);
}


#[test]
fn ordinary_slot_lookup_never_registers_or_substitutes_a_root() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let disk = temporary.path().join("primary");
    for path in [&system, &disk] {
        std::fs::create_dir(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = crate::Config::independent_roots(storage_contract::Mode::Development,
        [(name_policy::Name::parse("Primary").unwrap(), disk.clone())]).unwrap();
    let mut engine = crate::Engine::initialize(&system, config).unwrap();
    let empty = crate::canonical_json(engine.state()).unwrap();
    assert!(engine.ordinary_root_for_slot("d01").unwrap().is_none());
    assert!(engine.ordinary_root_for_slot("unknown").is_err());
    assert_eq!(crate::canonical_json(engine.state()).unwrap(), empty);

    let made = engine.create_disk_root("D01", "admin", crate::OperationId::generate().unwrap()).unwrap();
    let before = crate::canonical_json(engine.state()).unwrap();
    let selected = engine.ordinary_root_for_slot("d01").unwrap().unwrap();
    assert_eq!(selected.repository_id, made.repository_id);
    assert_eq!(selected.main_branch_id, made.main_branch_id);
    assert_eq!(crate::canonical_json(engine.state()).unwrap(), before);
    // A same-spelling replacement is not the held registered common root.
    let common = disk.join(crate::persist::layout::COMMON_DATA_ROOT.as_path());
    let retained = temporary.path().join("original-common");
    std::fs::rename(&common, &retained).unwrap();
    std::fs::create_dir(&common).unwrap();
    std::fs::set_permissions(&common, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(engine.ordinary_root_for_slot("D01").is_err());
    assert_eq!(crate::canonical_json(engine.state()).unwrap(), before);
    std::fs::remove_dir(&common).unwrap();
    std::fs::rename(retained, common).unwrap();
    drop(engine);
    let opened = crate::Engine::open(&system).unwrap();
    assert_eq!(opened.ordinary_root_for_slot("D01").unwrap().unwrap().repository_id,
        made.repository_id);
}

#[test]
fn disabled_ordinary_metadata_requires_the_original_registry_without_opening_stopped_media() {
    use crate::{CommitRequest, Engine, MembershipExitAction, OperationId};
    use std::fs;
    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let mut config = crate::Config::independent_roots(storage_contract::Mode::Development,
        crate::test_disks((0..4).map(|i| temporary.path().join(format!("disk-{i}")))),
    ).unwrap();
    config.disks[3].metadata_member = false;
    let disk = config.disks[3].clone();
    let op = || OperationId::generate().unwrap();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let original = engine.create_disk_root(&disk.slot_id, "admin", op()).unwrap();
    let work = engine.create_workspace(&original.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&work.workspace_id, "proof.zip", b"opaque file bytes").unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: work.workspace_id, operation_id: op(), principal: "admin".into(),
        expected_head: work.base_head, fencing_token: work.fencing_token,
        message: "held original FILE".into(), request_digest: None,
    }).unwrap();
    let entry = engine.namespace_tree(&original.main_branch_id).unwrap()
        .get("proof.zip").unwrap().clone();
    let original_id = original.repository_id;
    let exit = |engine: &mut Engine, action| {
        engine.begin_disk_exit(&disk.disk_id, action, "admin", op()).unwrap();
        let archive = engine.build_managed_membership_control_archive().unwrap();
        engine.enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new()).unwrap();
        engine.resume_membership_joint_decision().unwrap();
        engine.activate_pending_disk_exit().unwrap();
    };
    exit(&mut engine, MembershipExitAction::PlannedOffline);
    fs::rename(&disk.root, temporary.path().join("physically-stopped")).unwrap();
    let sentinel = b"this path is not a disk";
    fs::write(&disk.root, sentinel).unwrap();
    for reopen in [false, true] {
        if reopen { drop(engine); engine = Engine::open(&system).unwrap(); }
        let before = crate::canonical_json(engine.state()).unwrap();
        let registered = engine.ordinary_metadata_root_for_slot(&disk.slot_id).unwrap().unwrap();
        assert_eq!(registered.repository_id, original_id);
        let listed = engine.namespace_tree(&registered.main_branch_id).unwrap()
            .get("proof.zip").unwrap();
        assert_eq!(listed, &entry);
        let projected = engine.native_current_entries(registered.repository_id.parse().unwrap()).unwrap();
        assert!(matches!(&projected[..], [crate::NativeCurrentEntry::Published(row)] if row == &entry));
        assert!(engine.ordinary_root_for_slot(&disk.slot_id).is_err());
        assert!(engine.open_content(&entry.version_id).is_err());
        assert_eq!(fs::read(&disk.root).unwrap(), sentinel);
        assert_eq!(crate::canonical_json(engine.state()).unwrap(), before);
    }
    exit(&mut engine, MembershipExitAction::Deregister);
    assert!(engine.ordinary_metadata_root_for_slot(&disk.slot_id).is_err());
    assert_eq!(fs::read(&disk.root).unwrap(), sentinel);
}


#[test]
fn ordinary_basis_keeps_the_exact_head_and_requires_its_original_root_creation() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = crate::Engine::initialize(&temp.path().join("system"),
        crate::Config::development(crate::test_disks([temp.path().join("disk")])).unwrap()).unwrap();
    let make_root = crate::OperationId::generate().unwrap();
    let repo = engine.create_disk_root("P01", "admin", make_root).unwrap();
    let workspace = engine.create_workspace(&repo.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "folder/opaque.zip", b"ordinary FILE").unwrap();
    let save = engine.commit_workspace(crate::CommitRequest {
        workspace_id: workspace.workspace_id, operation_id: crate::OperationId::generate().unwrap(),
        principal: "admin".into(), expected_head: workspace.base_head, fencing_token: workspace.fencing_token,
        message: "ordinary root basis".into(), request_digest: None,
    }).unwrap();
    let repository = repo.repository_id.parse().unwrap();
    let original = crate::canonical_json(engine.state()).unwrap();
    let (head, entries) = crate::engine::fresh_layout::namespace::ordinary_root_basis(
        engine.state(), engine.admitted_current_namespaces().unwrap(), repository).unwrap();
    assert_eq!(Some(head), save.commit_id.as_deref());
    assert_eq!(entries, engine.list_tree_ref(&repo.main_branch_id).unwrap());
    assert_eq!(entries.len(), 2);
    let mut unproved = engine.state().clone();
    unproved.operations.remove(&make_root);
    assert!(crate::engine::fresh_layout::namespace::ordinary_root_basis(
        &unproved, None, repository).is_err());
    assert_eq!(crate::canonical_json(engine.state()).unwrap(), original);
}
