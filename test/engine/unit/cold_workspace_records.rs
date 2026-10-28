use super::*;

#[test]
fn cold_full_manifest_keeps_opened_file_selection_context_and_fencing() {
    let f = Fixture::new();
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    save(&mut engine, &repo.main_branch_id, "file", b"original");
    let mut expected = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    assert!(!expected.selected_file_contexts.is_empty());
    assert!(!expected.selected_file_branches.is_empty());
    let id = expected.workspace_id.clone();
    let path = name_policy::PortablePath::parse("file").unwrap();
    engine
        .validate_workspace_file_selection(&id, &path)
        .unwrap();
    engine.abort_workspace(&id, "admin").unwrap();
    expected.state = WorkspaceState::Aborted;
    expected.fencing_token += 1;
    let canonical = canonical_json(&expected).unwrap();
    assert_eq!(
        canonical_json(&engine.workspace(&id).unwrap()).unwrap(),
        canonical
    );
    engine
        .validate_workspace_file_selection(&id, &path)
        .unwrap();
    drop(engine);
    let mut engine = Engine::open(&f.root).unwrap();
    assert_eq!(
        canonical_json(&engine.workspace(&id).unwrap()).unwrap(),
        canonical
    );
    engine
        .validate_workspace_file_selection(&id, &path)
        .unwrap();
    let source = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let draft = engine
        .create_file_branch(
            &source.file_id,
            &source.version_id,
            &BranchName::parse("same bytes").unwrap(),
            "admin",
            OperationId::generate(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    engine
        .select_file_branch(&source.file_id, &draft, "admin", OperationId::generate())
        .unwrap();
    assert_eq!(
        engine
            .validate_workspace_file_selection(&id, &path)
            .unwrap_err()
            .code(),
        ErrorCode::FileBranchConflict
    );
    assert_eq!(
        canonical_json(&engine.workspace(&id).unwrap()).unwrap(),
        canonical
    );
    assert_eq!(
        engine.abort_workspace(&id, "foreign").unwrap_err().code(),
        ErrorCode::PermissionDenied
    );
}

#[test]
fn a_stray_primary_manifest_cannot_replace_the_authoritative_copy_for_cold_admission() {
    let mut f = Fixture::new();
    let other = f.temp.path().join("other-primary");
    f.config.disks.push(DiskConfig {
        slot_id: "P02".into(),
        disk_id: new_id("disk"),
        root: other.clone(),
        level: 1,
        upstream_slot_id: None,
        metadata_member: false,
        binding_generation: 1,
        capacity_bytes: None,
    });
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let primary = f.config.disks[0]
        .root
        .join("staging/workspaces")
        .join(&workspace.workspace_id)
        .join("workspace.json");
    let stray = other
        .join("staging/workspaces")
        .join(&workspace.workspace_id)
        .join("workspace.json");
    crate::disk::write_json(&stray, &workspace, false).unwrap();
    fs::remove_file(&primary).unwrap();
    assert_eq!(
        engine
            .abort_workspace(&workspace.workspace_id, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert!(!primary.exists());
    assert!(!engine.blocked);
    assert_eq!(
        engine.workspace(&workspace.workspace_id).unwrap().state,
        WorkspaceState::Open
    );
    assert_eq!(
        fs::read(stray).unwrap(),
        canonical_json(&workspace).unwrap()
    );
    assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 0);
}

#[test]
fn empty_aborts_release_only_resident_records_and_repeat_without_reactivation() {
    let mut f = Fixture::new();
    f.config.max_retained_workspaces = 1;
    f.config.max_retained_workspaces_per_principal = 1;
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let sequence = engine.state().sequence;
    let mut ids = Vec::new();
    for _ in 0..16 {
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .abort_workspace(&workspace.workspace_id, "admin")
            .unwrap();
        ids.push(workspace.workspace_id);
        assert!(engine.workspaces.is_empty());
        let capacity = engine.workspace_capacity("admin");
        assert_eq!(capacity.principal.records.used, 0);
        assert_eq!(capacity.principal.cold_records, ids.len() as u64);
        assert!(capacity.principal.metadata_bytes.used > 0);
    }
    let active = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let before = engine.workspace_capacity("admin");
    let closed = engine.workspace(&ids[0]).unwrap();
    assert!(matches!(
        engine.workspace_view(&ids[0]).unwrap(),
        std::borrow::Cow::Owned(_)
    ));
    assert!(matches!(
        engine.workspace_view(&active.workspace_id).unwrap(),
        std::borrow::Cow::Borrowed(_)
    ));
    engine.abort_workspace(&ids[0], "admin").unwrap();
    let repeated = engine.workspace(&ids[0]).unwrap();
    assert_eq!(repeated.fencing_token, closed.fencing_token + 1);
    assert_eq!(repeated.state, WorkspaceState::Aborted);
    assert_eq!(engine.workspace_capacity("admin"), before);
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(
        engine
            .abort_workspace(&ids[0], "foreign")
            .unwrap_err()
            .code(),
        ErrorCode::PermissionDenied
    );
    engine.assert_workspace_budget_consistent();
    drop(engine);
    let engine = Engine::open(&f.root).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), before);
    assert_eq!(engine.workspaces.len(), 1);
    assert_eq!(
        canonical_json(&engine.workspace(&ids[0]).unwrap()).unwrap(),
        canonical_json(&repeated).unwrap()
    );
    engine.assert_workspace_budget_consistent();
}

#[test]
fn cold_record_point_reads_reject_changed_or_conflicting_evidence() {
    for both in [false, true] {
        let f = Fixture::new();
        let mut engine = f.initialize();
        let repo = repository(&mut engine, 1);
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .abort_workspace(&workspace.workspace_id, "admin")
            .unwrap();
        let mut changed = engine.workspace(&workspace.workspace_id).unwrap();
        changed.fencing_token += 1;
        let paths = [
            f.root
                .join("workspaces")
                .join(&workspace.workspace_id)
                .join("workspace.json"),
            f.config.disks[0]
                .root
                .join("staging/workspaces")
                .join(&workspace.workspace_id)
                .join("workspace.json"),
        ];
        for path in paths.iter().take(if both { 2 } else { 1 }) {
            crate::disk::write_json(path, &changed, true).unwrap();
        }
        assert_eq!(
            engine
                .workspace(&workspace.workspace_id)
                .unwrap_err()
                .code(),
            ErrorCode::RecoveryRequired
        );
        assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 1);
        for path in paths.iter().take(if both { 2 } else { 1 }) {
            assert_eq!(fs::read(path).unwrap(), canonical_json(&changed).unwrap());
        }
    }
}

#[test]
fn missing_cold_mirror_requires_reopen_before_using_single_mirror_recovery_policy() {
    let f = Fixture::new();
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    let system = f
        .root
        .join("workspaces")
        .join(&workspace.workspace_id)
        .join("workspace.json");
    fs::remove_file(&system).unwrap();
    assert_eq!(
        engine
            .workspace(&workspace.workspace_id)
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        engine
            .abort_workspace(&workspace.workspace_id, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert!(!system.exists());
    drop(engine);
    let mut engine = Engine::open(&f.root).unwrap();
    assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 1);
    engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    assert!(system.is_file());
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 0);
}

#[test]
fn failed_pre_abort_classification_does_not_persist_abort_or_block_engine() {
    let f = Fixture::new();
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let primary = f.config.disks[0]
        .root
        .join("staging/workspaces")
        .join(&workspace.workspace_id);
    let retained = primary.with_extension("retained");
    fs::rename(&primary, &retained).unwrap();
    std::os::unix::fs::symlink(&retained, &primary).unwrap();
    assert!(
        engine
            .abort_workspace(&workspace.workspace_id, "admin")
            .is_err()
    );
    assert!(!engine.blocked);
    assert_eq!(
        engine.workspace(&workspace.workspace_id).unwrap().state,
        WorkspaceState::Open
    );
    let saved: Workspace = crate::disk::read_json(&retained.join("workspace.json")).unwrap();
    assert_eq!(saved.state, WorkspaceState::Open);
    let saved: Workspace = crate::disk::read_json(
        &f.root
            .join("workspaces")
            .join(&workspace.workspace_id)
            .join("workspace.json"),
    )
    .unwrap();
    assert_eq!(saved.state, WorkspaceState::Open);
    fs::remove_file(&primary).unwrap();
    fs::rename(&retained, &primary).unwrap();
    engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 1);
}

#[test]
fn cold_metadata_still_blocks_new_records_and_reserves_repeated_abort() {
    let f = Fixture::new();
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    let charged = engine
        .workspace_capacity("admin")
        .principal
        .metadata_bytes
        .used;
    let mut config = engine.config().clone();
    config.max_retained_workspace_metadata_bytes_per_principal = charged;
    engine.config = ValidatedConfig::try_from(config).unwrap();
    assert_eq!(
        engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimit
    );
    engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    let capacity = engine.workspace_capacity("admin");
    assert_eq!(capacity.principal.records.used, 0);
    assert_eq!(capacity.principal.cold_records, 1);
    assert_eq!(capacity.principal.metadata_bytes.used, charged);
    assert_eq!(capacity.warning, CapacityWarning::Exhausted);
}

#[test]
fn abort_does_not_evict_an_upload_or_detached_preparation_owner() {
    for upload in [false, true] {
        let f = Fixture::new();
        let mut engine = f.initialize();
        let repo = repository(&mut engine, 1);
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        if upload {
            let source = f.temp.path().join("source");
            fs::write(&source, b"pending").unwrap();
            let plan = engine
                .prepare_upload(
                    &workspace.workspace_id,
                    &new_id("entry"),
                    "file",
                    &source,
                    1,
                )
                .unwrap();
            engine
                .abort_workspace(&workspace.workspace_id, "admin")
                .unwrap();
            assert!(engine.workspaces.contains_key(&workspace.workspace_id));
            drop(plan);
        } else {
            let plan = engine
                .prepare_workspace_commit(
                    CommitRequest {
                        workspace_id: workspace.workspace_id.clone(),
                        operation_id: OperationId::generate(),
                        principal: "admin".into(),
                        expected_head: workspace.base_head,
                        fencing_token: workspace.fencing_token,
                        message: "pending".into(),
                        request_digest: None,
                    },
                    None,
                )
                .unwrap();
            engine
                .abort_workspace(&workspace.workspace_id, "admin")
                .unwrap();
            assert!(engine.workspaces.contains_key(&workspace.workspace_id));
            drop(plan);
        }
        assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 0);
        engine
            .abort_workspace(&workspace.workspace_id, "admin")
            .unwrap();
        assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 1);
        engine.assert_workspace_budget_consistent();
    }
}

#[test]
fn failed_abort_mirror_does_not_release_resident_quota_or_hide_evidence() {
    let f = Fixture::new();
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let system = f
        .root
        .join("workspaces")
        .join(&workspace.workspace_id)
        .join("workspace.json");
    fs::remove_file(&system).unwrap();
    fs::create_dir(&system).unwrap();
    assert!(
        engine
            .abort_workspace(&workspace.workspace_id, "admin")
            .is_err()
    );
    assert!(engine.blocked);
    assert!(engine.workspaces.contains_key(&workspace.workspace_id));
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
    assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 0);
    let primary = f.config.disks[0]
        .root
        .join("staging/workspaces")
        .join(&workspace.workspace_id)
        .join("workspace.json");
    let persisted: Workspace = crate::disk::read_json(&primary).unwrap();
    assert_eq!(persisted.state, WorkspaceState::Aborted);
    assert!(system.is_dir());
}

#[test]
fn commit_request_evidence_keeps_an_empty_abort_resident() {
    let f = Fixture::new();
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: OperationId::generate(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "unpublished".into(),
        request_digest: None,
    };
    let directory = f.config.disks[0]
        .root
        .join("staging/workspaces")
        .join(&workspace.workspace_id);
    crate::disk::write_json(&directory.join("commit-request.json"), &request, false).unwrap();
    engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
    drop(engine);
    let engine = Engine::open(&f.root).unwrap();
    assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 0);
    assert!(engine.workspaces.contains_key(&workspace.workspace_id));
}

#[test]
fn retained_directory_material_keeps_an_empty_abort_resident() {
    for name in ["old.full", "old.prepared", ".zxs-full-pending", "unknown"] {
        let f = Fixture::new();
        let mut engine = f.initialize();
        let repo = repository(&mut engine, 1);
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        let directory = f.config.disks[0]
            .root
            .join("staging/workspaces")
            .join(&workspace.workspace_id);
        fs::write(directory.join(name), b"retained bytes").unwrap();
        engine
            .abort_workspace(&workspace.workspace_id, "admin")
            .unwrap();
        assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
        assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 0);
        drop(engine);
        let engine = Engine::open(&f.root).unwrap();
        assert!(engine.workspaces.contains_key(&workspace.workspace_id));
        assert_eq!(fs::read(directory.join(name)).unwrap(), b"retained bytes");
    }
}

#[test]
fn cold_records_restore_from_primary_without_reentering_resident_quota() {
    let mut f = Fixture::new();
    f.config.max_retained_workspaces = 1;
    f.config.max_retained_workspaces_per_principal = 1;
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let mut ids = Vec::new();
    for _ in 0..8 {
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .abort_workspace(&workspace.workspace_id, "admin")
            .unwrap();
        ids.push(workspace.workspace_id);
    }
    let capacity = engine.workspace_capacity("admin");
    let sequence = engine.state().sequence;
    drop(engine);
    let restored = f.temp.path().join("cold-restored");
    let disks = f
        .config
        .disks
        .iter()
        .map(|disk| disk.root.clone())
        .collect::<Vec<_>>();
    let mut engine = Engine::restore_from_disks(&restored, &disks).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    assert_eq!(engine.state().sequence, sequence);
    assert!(engine.workspaces.is_empty());
    for id in &ids {
        assert_eq!(engine.workspace(id).unwrap().state, WorkspaceState::Aborted);
    }
    engine.abort_workspace(&ids[0], "admin").unwrap();
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
}

#[test]
fn inventory_streams_over_budget_manifests_without_opening_or_repairing_engine() {
    let mut f = Fixture::new();
    f.config.max_retained_workspace_metadata_bytes = 4096;
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    drop(engine);
    let mut smaller = f.config.clone();
    smaller.max_retained_workspace_metadata_bytes = 1;
    crate::disk::write_json(&f.root.join("config.json"), &smaller, true).unwrap();
    assert_eq!(
        Engine::open(&f.root).err().unwrap().code(),
        ErrorCode::ResourceLimit
    );
    let paths = [
        f.root
            .join("workspaces")
            .join(&workspace.workspace_id)
            .join("workspace.json"),
        f.config.disks[0]
            .root
            .join("staging/workspaces")
            .join(&workspace.workspace_id)
            .join("workspace.json"),
    ];
    let before = paths
        .iter()
        .map(|path| fs::read(path).unwrap())
        .collect::<Vec<_>>();
    let records = WorkspaceInventory::open(&f.root)
        .unwrap()
        .collect::<Result<Vec<_>>>()
        .unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|record| record.workspace_id == workspace.workspace_id
                && record.state == WorkspaceState::Aborted)
    );
    assert_eq!(
        paths
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect::<Vec<_>>(),
        before
    );
    assert_eq!(
        Engine::open(&f.root).err().unwrap().code(),
        ErrorCode::ResourceLimit
    );
}
