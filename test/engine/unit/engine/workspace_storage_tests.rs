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
    let mut candidate = historical::HistoricalWorkspaceCandidate::new(
        &recovered,
        &canonical_json(&recovered).unwrap(),
    );
    candidate
        .observe(&engine.state.plans[&plan.plan_id])
        .unwrap();
    assert!(candidate.reconcile(&mut recovered).unwrap());
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
        let mut check = || -> Result<bool> {
            let mut candidate = historical::HistoricalWorkspaceCandidate::new(
                &recovered,
                &canonical_json(&recovered)?,
            );
            candidate.observe(&engine.state.plans[&plan.plan_id])?;
            candidate.reconcile(&mut recovered)
        };
        let error = check().unwrap_err();
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
            let prepared = engine.prepare_recovery().unwrap();
            engine.seed_workspace_recovery(&prepared)
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
        .read_workspace_mirrors(
            &workspace.workspace_id,
            &paths,
            &locations,
            WorkspaceRead::Published,
            None,
        )
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

fn historical_storage_fixture() -> (tempfile::TempDir, Engine, Repository, String) {
    let root = tempfile::tempdir().unwrap();
    let config = Config::development([root.path().join("primary")]);
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("historical mirrors").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let source = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    (root, engine, repo, source)
}

fn historical_manifest_paths(engine: &Engine, id: &str) -> [PathBuf; 2] {
    [
        engine
            .root
            .join("workspaces")
            .join(id)
            .join("workspace.json"),
        engine.config.disks[0]
            .root
            .join("staging/workspaces")
            .join(id)
            .join("workspace.json"),
    ]
}

#[test]
fn historical_cold_point_read_rejects_changed_mirror_bytes_without_reconciliation_writes() {
    let (_root, mut engine, repo, source) = historical_storage_fixture();
    let workspace = engine
        .create_historical_workspace(&repo.main_branch_id, &source, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file", b"historical mirror bytes")
        .unwrap();
    let plan = engine
        .create_historical_save_plan(
            &repo.history_tree_id,
            crate::HistoricalSaveOptions {
                source_commit_id: source,
                name: "published historical mirror".into(),
                workspace_id: Some(workspace.workspace_id.clone()),
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    engine
        .commit_historical_save_plan(&plan.plan_id, "admin", OperationId::generate())
        .unwrap();
    let mut changed = engine.workspace(&workspace.workspace_id).unwrap();
    assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 1);
    changed.created_at += 1;
    let paths = historical_manifest_paths(&engine, &workspace.workspace_id);
    for path in &paths {
        write_json(path, &changed, false).unwrap();
    }
    let before = paths.each_ref().map(|path| fs::read(path).unwrap());
    let capacity = engine.workspace_capacity("admin");
    assert_eq!(
        engine
            .workspace(&workspace.workspace_id)
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(paths.each_ref().map(|path| fs::read(path).unwrap()), before);
    assert_eq!(engine.workspace_capacity("admin"), capacity);
}

#[test]
fn historical_seed_quota_rejection_preserves_config_journals_and_manifests() {
    for exhaust_bytes in [false, true] {
        let (_root, mut engine, repo, source) = historical_storage_fixture();
        let mut paths = vec![engine.root.join("config.json")];
        paths.extend(engine.config.disks.iter().map(crate::journal::log_path));
        for index in 0..2 {
            let mut workspace = engine
                .create_historical_workspace(&repo.main_branch_id, &source, "admin")
                .unwrap();
            engine
                .stage_bytes(
                    &workspace.workspace_id,
                    &format!("file-{index}"),
                    b"unpublished historical bytes",
                )
                .unwrap();
            workspace = engine.workspace(&workspace.workspace_id).unwrap();
            workspace.state = WorkspaceState::Committed;
            workspace.fencing_token += 1;
            let manifests = historical_manifest_paths(&engine, &workspace.workspace_id);
            for path in &manifests {
                write_json(path, &workspace, false).unwrap();
            }
            paths.extend(manifests);
        }
        let mut config = engine.config().clone();
        if exhaust_bytes {
            config.max_retained_workspace_metadata_bytes = 1;
            config.max_retained_workspace_metadata_bytes_per_principal = 1;
        } else {
            config.max_retained_workspaces = 1;
            config.max_retained_workspaces_per_principal = 1;
        }
        engine.config = super::super::ValidatedConfig::try_from(config).unwrap();
        let before: Vec<_> = paths.iter().map(|path| fs::read(path).unwrap()).collect();
        let prepared = engine.prepare_recovery().unwrap();
        let error = engine.seed_workspace_recovery(&prepared).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceLimit);
        assert_eq!(
            paths
                .iter()
                .map(|path| fs::read(path).unwrap())
                .collect::<Vec<_>>(),
            before
        );
    }
}

fn completed_historical_binding_fixture() -> (tempfile::TempDir, Engine, Workspace, Plan) {
    let (root, mut engine, repo, source) = historical_storage_fixture();
    let workspace = engine
        .create_historical_workspace(&repo.main_branch_id, &source, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file", b"binding bytes")
        .unwrap();
    let plan = engine
        .create_historical_save_plan(
            &repo.history_tree_id,
            crate::HistoricalSaveOptions {
                source_commit_id: source,
                name: "binding".into(),
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
    let completed = engine.state.plans[&plan.plan_id].clone();
    (root, engine, sealed, completed)
}

#[test]
fn historical_completion_binding_merges_duplicates_and_rejects_conflicting_plans() {
    let (_root, _engine, sealed, plan) = completed_historical_binding_fixture();
    let mut candidate =
        historical::HistoricalWorkspaceCandidate::new(&sealed, &canonical_json(&sealed).unwrap());
    let mut foreign = plan.clone();
    foreign.principal = "foreign".into();
    foreign.input["workspace_fencing_token"] = json!("broken");
    candidate.observe(&foreign).unwrap();
    assert!(!candidate.has_completion());
    candidate.observe(&plan).unwrap();
    candidate.observe(&plan).unwrap();
    let mut recovered = sealed.clone();
    assert!(candidate.reconcile(&mut recovered).unwrap());
    assert_eq!(recovered.state, WorkspaceState::Committed);
    let mut conflicting = plan.clone();
    conflicting.plan_id = new_id("plan");
    conflicting.input["workspace_fencing_token"] = json!((sealed.fencing_token + 1).to_string());
    assert_eq!(
        candidate.observe(&conflicting).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
}

#[test]
fn historical_published_sealed_mirrors_remain_resident_recovery_material() {
    let (root, engine, sealed, _plan) = completed_historical_binding_fixture();
    let paths = historical_manifest_paths(&engine, &sealed.workspace_id);
    for path in &paths {
        write_json(path, &sealed, false).unwrap();
    }
    let before = paths.each_ref().map(|path| fs::read(path).unwrap());
    drop(engine);
    let engine = Engine::open(&root.path().join("system")).unwrap();
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
    assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 0);
    let recovered = engine.workspace(&sealed.workspace_id).unwrap();
    assert_eq!(recovered.state, WorkspaceState::Committed);
    assert_eq!(recovered.fencing_token, sealed.fencing_token + 1);
    assert_eq!(paths.each_ref().map(|path| fs::read(path).unwrap()), before);
    for file in sealed.files.values() {
        assert_eq!(fs::read(&file.source_path).unwrap(), b"binding bytes");
    }
}

#[test]
fn historical_unpublished_completion_mirrors_are_only_seed_hints() {
    let (_root, mut engine, repo, source) = historical_storage_fixture();
    let workspace = engine
        .create_historical_workspace(&repo.main_branch_id, &source, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file", b"unpublished staged bytes")
        .unwrap();
    engine
        .seal_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    let mut completed = engine.workspace(&workspace.workspace_id).unwrap();
    completed.state = WorkspaceState::Committed;
    completed.fencing_token += 1;
    let paths = historical_manifest_paths(&engine, &workspace.workspace_id);
    write_json(&paths[1], &completed, false).unwrap();
    let before = paths.each_ref().map(|path| fs::read(path).unwrap());
    let prepared = engine.prepare_recovery().unwrap();
    engine.seed_workspace_recovery(&prepared).unwrap();
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
    assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 0);
    assert_eq!(
        engine.load_workspaces().unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(paths.each_ref().map(|path| fs::read(path).unwrap()), before);
}
