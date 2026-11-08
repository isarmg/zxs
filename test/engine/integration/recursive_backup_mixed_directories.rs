//! Explicit mixed directory snapshots use real bodies, native I/O and cold replay.
use super::*;

fn configure_tree(
    fixture: &mut Fixture,
    path: &str,
    capture: &RecursiveBackupCapture,
    generation: u64,
    id: OperationId,
) -> Result<OperationResult> {
    let scope = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, path)?
        .scope;
    fixture
        .engine
        .configure_recursive_backup_tree(RecursiveBackupConfigurationRequest {
            repository_id: &fixture.repository.repository_id,
            path,
            expected_scope_id: &scope.id,
            target_copies: 3,
            expected_generation: generation,
            capture,
            principal: "admin",
            operation_id: id,
        })
}

#[test]
fn mixed_snapshot_keeps_old_mode_and_follows_with_cold_repair_and_permanent_retry() {
    let mut fixture = Fixture::new(false);
    let first = fixture.save("root/docs/report", b"first retained");
    let latest = fixture.save("root/docs/report", b"latest retained");
    mkdir(&mut fixture, "root/empty");
    mkdir(&mut fixture, "root/empty/child");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let old = fixture.configure("root", &capture, 0, operation()).unwrap();
    assert_eq!(old.kind, "backup.configure-recursive");
    assert!(old.result.get("directory_namespace").is_none());
    assert!(names(&fixture, "C01").is_empty());

    let directory_ids = ["root", "root/docs", "root/empty", "root/empty/child"]
        .map(|path| {
            let scope = fixture
                .engine
                .backup_configuration(&fixture.repository.repository_id, path)
                .unwrap()
                .scope;
            (path.to_owned(), scope.id)
        })
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let versions = fixture.engine.state().versions.clone();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let repository = fixture.repository.repository_id.clone();
    let request = RecursiveBackupConfigurationRequest {
        repository_id: &repository,
        path: "root",
        expected_scope_id: &directory_ids["root"],
        target_copies: 3,
        expected_generation: 1,
        capture: &capture,
        principal: "admin",
        operation_id: operation(),
    };
    let receipt = fixture
        .engine
        .configure_recursive_backup_tree(request)
        .unwrap();
    assert_eq!(receipt.kind, "backup.configure-recursive-tree");
    assert_eq!(receipt.commit_state, CommitState::Published);
    assert_eq!(
        receipt.protection_at_publish,
        ProtectionAtPublish::TargetMet
    );
    assert_eq!(fixture.engine.state().versions, versions);
    for (disk, slot, prefix) in [
        (1, "B01", "disk-0/Repository"),
        (2, "C01", "disk-1/disk-0/Repository"),
    ] {
        let rows = fixture.engine.published_backup_directories(slot).unwrap();
        assert_eq!(rows.len(), directory_ids.len());
        for (path, id) in &directory_ids {
            let path = format!("{prefix}/{path}");
            let row = rows.iter().find(|row| row.path().as_str() == path).unwrap();
            assert_eq!(row.directory_id(), id);
            assert_eq!(row.operation_id(), receipt.operation_id);
            assert!(physical(&fixture, disk, receipt.operation_id, &path).is_dir());
        }
    }
    fixture.assert_body_paths(&first, "root/docs/report", b"first retained");
    fixture.assert_body_paths(&latest, "root/docs/report", b"latest retained");
    assert_eq!(fixture.engine.operation(old.operation_id).unwrap(), old);
    assert_eq!(
        fixture
            .engine
            .configure_recursive_backups(request)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );

    let original_files = files(&physical(&fixture, 2, receipt.operation_id, ""));
    mkdir(&mut fixture, "root/new-not-captured");
    let moved = fixture
        .engine
        .file_namespace(
            &repository,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, "root", Some("moved"))
                .unwrap(),
            "admin",
            operation(),
        )
        .unwrap();
    assert_eq!(moved.kind, "file.namespace.recursive-routes");
    assert_eq!(
        names(&fixture, "C01"),
        [
            "disk-1/disk-0/Repository/moved",
            "disk-1/disk-0/Repository/moved/docs",
            "disk-1/disk-0/Repository/moved/empty",
            "disk-1/disk-0/Repository/moved/empty/child",
        ]
    );
    let missing = physical(
        &fixture,
        2,
        moved.operation_id,
        "disk-1/disk-0/Repository/moved/empty/child",
    );
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fs::remove_dir(&missing).unwrap(); // Only this test's regenerable projection.
    fixture.engine = Engine::open(&system).unwrap();
    assert!(missing.is_dir());
    fixture.assert_body_paths(&first, "moved/docs/report", b"first retained");
    fixture.assert_body_paths(&latest, "moved/docs/report", b"latest retained");
    assert_eq!(
        files(&physical(&fixture, 2, receipt.operation_id, "")),
        original_files
    );
    assert_eq!(fixture.engine.operation(old.operation_id).unwrap(), old);
    assert_eq!(
        fixture.engine.operation(receipt.operation_id).unwrap(),
        receipt
    );
    assert_eq!(
        fixture
            .engine
            .configure_recursive_backup_tree(request)
            .unwrap(),
        receipt
    );
}

#[test]
fn mixed_snapshot_preserves_independent_child_policy_and_target_routes() {
    let mut fixture = Fixture::new(true);
    let file = fixture.save("outer/report", b"selected body");
    mkdir(&mut fixture, "outer/empty");
    mkdir(&mut fixture, "outer/independent");
    mkdir(&mut fixture, "outer/independent/child");
    let scope = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, "outer/independent")
        .unwrap()
        .scope;
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let child = fixture
        .engine
        .configure_recursive_backups(RecursiveBackupConfigurationRequest {
            repository_id: &fixture.repository.repository_id,
            path: "outer/independent",
            expected_scope_id: &scope.id,
            target_copies: 2,
            expected_generation: 0,
            capture: &capture,
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    let child_policy = fixture.engine.state().protection_policies
        [child.result["policy"]["policy_id"].as_str().unwrap()]
    .clone();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    configure_tree(&mut fixture, "outer", &capture, 0, operation()).unwrap();
    assert_eq!(
        names(&fixture, "B01"),
        [
            "disk-0/outer",
            "disk-0/outer/empty",
            "disk-0/outer/independent",
            "disk-0/outer/independent/child",
        ]
    );
    assert_eq!(
        names(&fixture, "C01"),
        ["disk-1/disk-0/outer", "disk-1/disk-0/outer/empty"]
    );
    assert_eq!(
        fixture.engine.state().protection_policies[&child_policy.policy_id],
        child_policy
    );
    assert_eq!(fixture.engine.operation(child.operation_id).unwrap(), child);
    fixture.assert_body_paths(&file, "outer/report", b"selected body");
}

#[test]
fn mixed_snapshot_rejects_data_only_targets_without_downgrading_or_changing_old_mode() {
    let mut fixture = ready_data_only_backup();
    let file = fixture.save("docs/report", b"data-only backup is still supported");
    mkdir(&mut fixture, "docs/empty");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let old = fixture.configure("docs", &capture, 0, operation()).unwrap();
    fixture.assert_body_paths(&file, "docs/report", b"data-only backup is still supported");
    let before = storage_engine::canonical_json(fixture.engine.state()).unwrap();
    let disk_bytes = fixture
        .config
        .disks
        .iter()
        .map(|disk| files(&disk.root))
        .collect::<Vec<_>>();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let id = operation();
    assert_eq!(
        configure_tree(&mut fixture, "docs", &capture, 1, id)
            .unwrap_err()
            .code(),
        ErrorCode::OperationUnsupported
    );
    assert_eq!(
        storage_engine::canonical_json(fixture.engine.state()).unwrap(),
        before
    );
    for (index, expected) in disk_bytes.iter().enumerate() {
        assert_eq!(&files(&fixture.config.disks[index].root), expected);
        assert!(!physical(&fixture, index, id, "").exists());
    }
    assert_eq!(fixture.engine.operation(old.operation_id).unwrap(), old);
}

#[test]
fn mixed_snapshot_rejects_file_scope_without_consuming_the_request() {
    let mut fixture = Fixture::new(false);
    fixture.save("report", b"file only");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let before = storage_engine::canonical_json(fixture.engine.state()).unwrap();
    let id = operation();
    assert_eq!(
        configure_tree(&mut fixture, "report", &capture, 0, id)
            .unwrap_err()
            .code(),
        ErrorCode::OperationUnsupported
    );
    assert_eq!(
        storage_engine::canonical_json(fixture.engine.state()).unwrap(),
        before
    );
    assert!(!fixture.engine.state().operations.contains_key(&id));
    assert_eq!(
        fixture.configure("report", &capture, 0, id).unwrap().kind,
        "backup.configure-recursive"
    );
}

#[test]
fn mixed_snapshot_io_failure_after_decision_cold_finishes_both_body_and_directory() {
    let mut fixture = Fixture::new(false);
    let file = fixture.save("docs/report", b"full file copy");
    mkdir(&mut fixture, "docs/empty");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let id = operation();
    let blocked = physical(&fixture, 1, id, "");
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(blocked.parent().unwrap())
        .unwrap();
    fs::write(&blocked, b"test-owned obstruction").unwrap();
    assert_eq!(
        configure_tree(&mut fixture, "docs", &capture, 0, id)
            .unwrap_err()
            .code(),
        ErrorCode::InDoubt
    );
    assert!(!fixture.engine.state().operations.contains_key(&id));
    assert_eq!(fs::read(&blocked).unwrap(), b"test-owned obstruction");
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fs::remove_file(&blocked).unwrap();
    fixture.engine = Engine::open(&system).unwrap();
    let receipt = fixture.engine.operation(id).unwrap();
    assert_eq!(receipt.kind, "backup.configure-recursive-tree");
    assert_eq!(receipt.commit_state, CommitState::Published);
    fixture.assert_body_paths(&file, "docs/report", b"full file copy");
    assert!(physical(&fixture, 2, id, "disk-1/disk-0/Repository/docs/empty").is_dir());
    assert_eq!(
        configure_tree(&mut fixture, "docs", &capture, 0, id).unwrap(),
        receipt
    );
}
