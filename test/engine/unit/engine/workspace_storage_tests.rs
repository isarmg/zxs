use super::*;
use json_codec::json;
use std::os::unix::fs::PermissionsExt;

#[test]
fn historical_completion_rejects_invalid_persisted_fence_instead_of_leaving_workspace_open() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("primary"),
        temp.path().join("backup"),
        temp.path().join("tertiary"),
    ]);
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("history").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let head = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let workspace = engine
        .create_historical_workspace(&repo.main_branch_id, &head, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file", b"historical bytes")
        .unwrap();
    let plan = engine
        .create_historical_save_plan(
            &repo.history_tree_id,
            crate::HistoricalSaveOptions {
                source_commit_id: head,
                name: "saved branch".into(),
                workspace_id: Some(workspace.workspace_id.clone()),
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    let sealed = engine.workspace(&workspace.workspace_id).unwrap();
    engine
        .commit_historical_save_plan(&plan.plan_id, "admin", OperationId::generate())
        .unwrap();
    let mut recovered = sealed.clone();
    assert!(engine.reconcile_workspace_receipt(&mut recovered).unwrap());
    assert_eq!(recovered.state, WorkspaceState::Committed);
    for invalid in [
        json!(null),
        json!("+1"),
        json!("01"),
        json!("garbage"),
        json!("0"),
        json!("18446744073709551615"),
        json!("18446744073709551616"),
    ] {
        Arc::make_mut(&mut engine.state)
            .plans
            .get_mut(&plan.plan_id)
            .unwrap()
            .input["workspace_fencing_token"] = invalid.clone();
        let mut recovered = sealed.clone();
        let error = engine
            .reconcile_workspace_receipt(&mut recovered)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::RecoveryRequired, "{invalid}");
        assert_eq!(recovered.state, sealed.state);
        assert_eq!(recovered.fencing_token, sealed.fencing_token);
    }
}

#[test]
fn workspace_evidence_distinguishes_absent_leaf_and_parent_from_json_damage() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let held = DiskRoot::open(root.path()).unwrap();
    let mut damage = None;
    for path in [
        root.path().join("absent.json"),
        root.path().join("absent/record.json"),
    ] {
        assert!(
            read_workspace_evidence::<json_codec::Value>(held.open_regular(&path), &mut damage)
                .unwrap()
                .is_none()
        );
        assert!(damage.is_none());
    }
    let path = root.path().join("record.json");
    fs::write(&path, b"{").unwrap();
    assert!(
        read_workspace_evidence::<json_codec::Value>(held.open_regular(&path), &mut damage)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        damage,
        Some(Error::Contract(storage_contract::Error::Json { .. }))
    ));
    fs::write(&path, b"{\"healthy\":true}").unwrap();
    assert_eq!(
        read_workspace_evidence::<json_codec::Value>(held.open_regular(&path), &mut damage)
            .unwrap(),
        Some(json!({"healthy":true}))
    );
}

#[test]
fn workspace_scans_reject_unsafe_private_directory_without_chmod_or_manifest_changes() {
    use platform_io::fs_safety::Error as FsError;
    for authoritative in [false, true] {
        let (_temp, mut engine, workspace) = workspace_fixture(1);
        let directory = engine.config.disks[0].root.join("staging/workspaces");
        let manifests = [
            engine
                .root
                .join("workspaces")
                .join(&workspace.workspace_id)
                .join("workspace.json"),
            directory
                .join(&workspace.workspace_id)
                .join("workspace.json"),
        ];
        let before = manifests.each_ref().map(|path| fs::read(path).unwrap());
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        let scan = if authoritative {
            engine.load_workspaces()
        } else {
            engine.seed_workspace_recovery()
        };
        assert!(matches!(
            scan,
            Err(Error::Filesystem(FsError::UnsafePermissions(rejected))) if rejected == directory
        ));
        assert_eq!(
            manifests.each_ref().map(|path| fs::read(path).unwrap()),
            before
        );
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o7777,
            0o755
        );
    }
}

#[test]
fn workspace_inventory_rejects_unsafe_private_directory_without_chmod_or_manifest_changes() {
    use platform_io::fs_safety::Error as FsError;
    let (_temp, engine, workspace) = workspace_fixture(1);
    let root = engine.root.clone();
    let directory = engine.config.disks[0].root.join("staging/workspaces");
    let manifests = [
        root.join("workspaces")
            .join(&workspace.workspace_id)
            .join("workspace.json"),
        directory
            .join(&workspace.workspace_id)
            .join("workspace.json"),
    ];
    let before = manifests.each_ref().map(|path| fs::read(path).unwrap());
    drop(engine);
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    let scan = WorkspaceInventory::open(&root);
    assert!(matches!(
        scan,
        Err(Error::Filesystem(FsError::UnsafePermissions(rejected))) if rejected == directory
    ));
    assert_eq!(
        manifests.each_ref().map(|path| fs::read(path).unwrap()),
        before
    );
    assert_eq!(
        fs::metadata(directory).unwrap().permissions().mode() & 0o7777,
        0o755
    );
}

#[test]
fn cold_point_read_rejects_unsafe_private_base_without_chmod_or_manifest_changes() {
    use platform_io::fs_safety::Error as FsError;
    let (_temp, mut engine, workspace) = workspace_fixture(1);
    engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    let directory = engine.config.disks[0].root.join("staging/workspaces");
    let manifest = directory
        .join(&workspace.workspace_id)
        .join("workspace.json");
    let before = fs::read(&manifest).unwrap();
    let capacity = engine.workspace_capacity("admin");
    assert_eq!(capacity.principal.cold_records, 1);
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        engine.workspace(&workspace.workspace_id),
        Err(Error::Filesystem(FsError::UnsafePermissions(rejected))) if rejected == directory
    ));
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    assert_eq!(fs::read(manifest).unwrap(), before);
    assert_eq!(
        fs::metadata(directory).unwrap().permissions().mode() & 0o7777,
        0o755
    );
}

#[test]
fn workspace_scan_reads_the_admitted_private_base_after_path_replacement() {
    let (_temp, engine, workspace) = workspace_fixture(1);
    let locations = engine.workspace_locations().unwrap();
    let location = locations
        .iter()
        .find(|location| matches!(location, WorkspaceLocation::Primary(_)))
        .unwrap();
    let directory = location.directory();
    let moved = directory.with_extension("held");
    fs::rename(&directory, &moved).unwrap();
    private_dir(&directory).unwrap();
    let replacement = directory
        .join(&workspace.workspace_id)
        .join("workspace.json");
    let mut unrelated = workspace.clone();
    unrelated.principal = "replacement-primary".into();
    write_json(&replacement, &unrelated, false).unwrap();
    let untouched = fs::read(&replacement).unwrap();
    let entries = location
        .root()
        .entries(&directory)
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].as_os_str(), workspace.workspace_id.as_str());
    let paths = workspace_paths(&locations, &workspace.workspace_id).unwrap();
    let loaded = engine
        .read_workspace_mirrors(&workspace.workspace_id, &paths, true)
        .unwrap();
    assert_eq!(loaded.canonical, canonical_json(&workspace).unwrap());
    assert_eq!(fs::read(replacement).unwrap(), untouched);
}

fn workspace_fixture(disks: usize) -> (tempfile::TempDir, Engine, Workspace) {
    let temp = tempfile::tempdir().unwrap();
    let config =
        Config::development((0..disks).map(|index| temp.path().join(format!("disk-{index}"))));
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("workspace mirrors").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    (temp, engine, workspace)
}

#[test]
fn workspace_mirrors_keep_the_admitted_resource_after_root_replacement() {
    let (_temp, mut engine, mut workspace) = workspace_fixture(1);
    let disk = engine.config.disks[0].clone();
    let moved = disk.root.with_extension("held");
    fs::rename(&disk.root, &moved).unwrap();
    private_dir(&disk.root).unwrap();
    let replacement = disk
        .root
        .join("staging/workspaces")
        .join(&workspace.workspace_id)
        .join("workspace.json");
    let mut unrelated = workspace.clone();
    unrelated.principal = "replacement-disk".into();
    write_json(&replacement, &unrelated, false).unwrap();
    let untouched = fs::read(&replacement).unwrap();

    workspace.fencing_token += 1;
    engine.persist_workspace(&workspace).unwrap();
    engine.load_workspaces().unwrap();
    assert_eq!(
        engine
            .workspace(&workspace.workspace_id)
            .unwrap()
            .fencing_token,
        workspace.fencing_token
    );
    assert_eq!(
        engine.workspace(&workspace.workspace_id).unwrap().principal,
        "admin"
    );
    let held: Workspace = read_json(
        &moved
            .join("staging/workspaces")
            .join(&workspace.workspace_id)
            .join("workspace.json"),
    )
    .unwrap();
    assert_eq!(held.fencing_token, workspace.fencing_token);
    assert_eq!(fs::read(replacement).unwrap(), untouched);
}

#[test]
fn workspace_recovery_does_not_adopt_a_mirror_on_an_unadmitted_replacement() {
    let (temp, engine, workspace) = workspace_fixture(3);
    let disk = engine.config.disks[0].clone();
    drop(engine);
    fs::rename(&disk.root, disk.root.with_extension("missing")).unwrap();
    private_dir(&disk.root).unwrap();
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert!(engine.disk_resources.available(&disk).is_none());
    assert!(engine.blocked);
    let replacement = disk
        .root
        .join("staging/workspaces")
        .join(&workspace.workspace_id)
        .join("workspace.json");
    let mut unrelated = workspace.clone();
    unrelated.principal = "unadmitted".into();
    write_json(&replacement, &unrelated, false).unwrap();

    engine.load_workspaces().unwrap();
    assert_eq!(
        engine.workspace(&workspace.workspace_id).unwrap().principal,
        "admin"
    );
    let directories = engine
        .planning_context()
        .workspace_evidence_directories(&workspace)
        .unwrap();
    assert_eq!(directories.len(), 1);
    assert_eq!(
        directories[0].path(),
        engine.root.join("workspaces").join(&workspace.workspace_id)
    );
    assert!(
        engine.blocked,
        "missing members do not shrink the write barrier"
    );
}

#[test]
fn readonly_healthy_mirrors_reuse_exact_unmodified_canonical_evidence() {
    let (_temp, engine, workspace) = workspace_fixture(1);
    let locations = engine.workspace_locations().unwrap();
    let paths = workspace_paths(&locations, &workspace.workspace_id).unwrap();
    assert_eq!(paths.len(), 2);
    let before: Vec<_> = paths
        .iter()
        .map(|source| fs::read(source.path()).unwrap())
        .collect();
    let loaded = engine
        .read_workspace_mirrors(&workspace.workspace_id, &paths, false)
        .unwrap();
    assert!(loaded.consistent);
    assert!(!loaded.terminal);
    assert_eq!(loaded.canonical, canonical_json(&workspace).unwrap());
    assert_eq!(loaded.canonical, canonical_json(&loaded.workspace).unwrap());
    for (source, bytes) in paths.iter().zip(before) {
        assert_eq!(fs::read(source.path()).unwrap(), bytes);
    }
}

#[test]
fn readonly_mirror_reuse_does_not_hide_conflicting_owner_or_unproven_completion() {
    let (_temp, engine, workspace) = workspace_fixture(1);
    let locations = engine.workspace_locations().unwrap();
    let paths = workspace_paths(&locations, &workspace.workspace_id).unwrap();
    assert_eq!(paths.len(), 2);
    let mut conflicting = workspace.clone();
    conflicting.principal = "different-owner".into();
    write_json(paths[1].path(), &conflicting, true).unwrap();
    let before = fs::read(paths[1].path()).unwrap();
    let error = engine
        .read_workspace_mirrors(&workspace.workspace_id, &paths, false)
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert_eq!(fs::read(paths[1].path()).unwrap(), before);

    let mut completed = workspace.clone();
    completed.state = WorkspaceState::Committed;
    completed.fencing_token += 1;
    write_json(paths[1].path(), &completed, true).unwrap();
    let loaded = engine
        .read_workspace_mirrors(&workspace.workspace_id, &paths, false)
        .unwrap();
    assert!(!loaded.consistent);
    assert_eq!(loaded.canonical, canonical_json(&workspace).unwrap());
    assert_eq!(loaded.workspace.state, WorkspaceState::Open);
    // Read-only preflight may defer this possible transition; reconciliation
    // still rejects it without the exact publication receipt.
    let error = engine
        .read_workspace_mirrors(&workspace.workspace_id, &paths, true)
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert_eq!(
        fs::read(paths[1].path()).unwrap(),
        canonical_json(&completed).unwrap()
    );
}
