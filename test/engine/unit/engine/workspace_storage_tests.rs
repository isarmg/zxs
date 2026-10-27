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
            read_workspace_evidence::<json_codec::Value>(
                held.open_regular(&path).map(Some),
                &mut damage
            )
            .unwrap()
            .is_none()
        );
        assert!(damage.is_none());
    }
    let path = root.path().join("record.json");
    fs::write(&path, b"{").unwrap();
    assert!(
        read_workspace_evidence::<json_codec::Value>(
            held.open_regular(&path).map(Some),
            &mut damage
        )
        .unwrap()
        .is_none()
    );
    assert!(matches!(
        damage,
        Some(Error::Contract(storage_contract::Error::Json { .. }))
    ));
    fs::write(&path, b"{\"healthy\":true}").unwrap();
    assert_eq!(
        read_workspace_evidence::<json_codec::Value>(
            held.open_regular(&path).map(Some),
            &mut damage
        )
        .unwrap(),
        Some(json!({"healthy":true}))
    );
}

#[test]
fn workspace_scan_rejects_unsafe_private_directory_without_chmod() {
    use platform_io::fs_safety::Error as FsError;
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let held = DiskRoot::open(root.path()).unwrap();
    let directory = root.path().join("staging/workspaces");
    private_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    let mut candidates = BTreeMap::new();
    assert!(matches!(
        collect_workspace_paths(&held, &ReadSource::external(directory.clone()), 1, &mut candidates),
        Err(Error::Filesystem(FsError::UnsafePermissions(rejected))) if rejected == directory
    ));
    assert!(candidates.is_empty());
    assert_eq!(
        fs::metadata(directory).unwrap().permissions().mode() & 0o7777,
        0o755
    );
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
