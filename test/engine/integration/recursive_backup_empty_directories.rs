//! Real public Engine/native I/O and cold recovery, not power-loss certification.
use super::*;

#[path = "recursive_backup_mixed_directories.rs"]
mod mixed;

fn mkdir(fixture: &mut Fixture, path: &str) {
    fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, path, None).unwrap(),
            "admin",
            operation(),
        )
        .unwrap();
}

fn names(fixture: &Fixture, slot: &str) -> Vec<String> {
    fixture
        .engine
        .published_backup_directories(slot)
        .unwrap()
        .iter()
        .map(|row| row.path().as_str().to_owned())
        .collect()
}

fn physical(
    fixture: &Fixture,
    index: usize,
    operation: OperationId,
    path: &str,
) -> std::path::PathBuf {
    let namespace = fixture.config.disks[index]
        .root
        .join("histories")
        .join(&fixture.repository.history_tree_id)
        .join("directory-backups")
        .join(operation.to_string())
        .join("namespace");
    if path.is_empty() {
        namespace
    } else {
        namespace.join(path)
    }
}

fn ready_data_only_backup() -> Fixture {
    let temp = tempfile::Builder::new()
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .tempdir()
        .unwrap();
    let mut config =
        Config::development((0..4).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    config.disks[1].metadata_member = false;
    config.disks[3].level = 1;
    config.disks[3].upstream_slot_id = None;
    config.disks[3].metadata_member = false;
    let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("Repository").unwrap(),
            "P01",
            1,
            "admin",
            operation(),
        )
        .unwrap();
    let mut fixture = Fixture {
        temp,
        config,
        engine,
        repository,
    };
    fixture.exit(3, MembershipExitAction::Deregister);
    fixture
}

#[test]
fn empty_directories_have_actual_two_hop_trees_permanent_retries_and_cold_reads() {
    let mut fixture = Fixture::new(false);
    mkdir(&mut fixture, "empty");
    mkdir(&mut fixture, "empty/child");
    let versions = fixture.engine.state().versions.clone();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let receipt = fixture
        .configure("empty", &capture, 0, operation())
        .unwrap();
    assert_eq!(receipt.commit_state, CommitState::Published);
    assert_eq!(
        receipt.protection_at_publish,
        ProtectionAtPublish::NotApplicable
    );
    assert_eq!(fixture.engine.state().versions, versions);
    assert!(fixture.engine.state().file_backup_paths.is_empty());
    for (index, slot, prefix) in [
        (1, "B01", "disk-0/Repository/empty"),
        (2, "C01", "disk-1/disk-0/Repository/empty"),
    ] {
        assert_eq!(
            names(&fixture, slot),
            [prefix.to_owned(), format!("{prefix}/child")]
        );
        assert!(physical(&fixture, index, receipt.operation_id, prefix).is_dir());
        assert!(
            physical(
                &fixture,
                index,
                receipt.operation_id,
                &format!("{prefix}/child")
            )
            .is_dir()
        );
    }
    assert_eq!(
        fixture
            .configure("empty", &capture, 0, receipt.operation_id)
            .unwrap(),
        receipt
    );
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(
        fixture.engine.operation(receipt.operation_id).unwrap(),
        receipt
    );
    assert_eq!(names(&fixture, "C01").len(), 2);
    assert_eq!(
        fixture
            .configure("empty", &capture, 0, receipt.operation_id)
            .unwrap(),
        receipt
    );
}

#[test]
fn cold_reopen_rebuilds_a_missing_directory_projection_after_later_publications() {
    let mut fixture = Fixture::new(true);
    mkdir(&mut fixture, "空目录");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let receipt = fixture
        .configure("空目录", &capture, 0, operation())
        .unwrap();
    // A later ordinary namespace publication makes this a prefix operation.
    mkdir(&mut fixture, "unrelated");
    let projection = physical(&fixture, 2, receipt.operation_id, "disk-1/disk-0/空目录");
    assert!(projection.is_dir());
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fs::remove_dir(&projection).unwrap(); // Only this test's regenerable empty cache.
    fixture.engine = Engine::open(&system).unwrap();
    assert!(projection.is_dir());
    assert_eq!(names(&fixture, "C01"), ["disk-1/disk-0/空目录"]);
    assert_eq!(
        fixture.engine.operation(receipt.operation_id).unwrap(),
        receipt
    );
}

#[test]
fn directory_io_failure_after_decision_stays_in_doubt_and_cold_recovery_finishes_it() {
    let mut fixture = Fixture::new(false);
    mkdir(&mut fixture, "empty");
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
    let error = fixture.configure("empty", &capture, 0, id).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InDoubt);
    assert!(!fixture.engine.state().operations.contains_key(&id));
    assert_eq!(fs::read(&blocked).unwrap(), b"test-owned obstruction");
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fs::remove_file(&blocked).unwrap(); // Release this test's injected I/O obstruction.
    fixture.engine = Engine::open(&system).unwrap();
    let receipt = fixture.engine.operation(id).unwrap();
    assert_eq!(receipt.commit_state, CommitState::Published);
    assert!(physical(&fixture, 1, id, "disk-0/Repository/empty").is_dir());
    assert!(physical(&fixture, 2, id, "disk-1/disk-0/Repository/empty").is_dir());
    assert_eq!(
        fixture.configure("empty", &capture, 0, id).unwrap(),
        receipt
    );
}

#[test]
fn data_only_directory_targets_are_rejected_without_state_or_disk_changes() {
    let mut fixture = ready_data_only_backup();
    mkdir(&mut fixture, "empty");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let before = storage_engine::canonical_json(fixture.engine.state()).unwrap();
    let disk_bytes = fixture
        .config
        .disks
        .iter()
        .map(|disk| files(&disk.root))
        .collect::<Vec<_>>();
    let id = operation();
    assert_eq!(
        fixture
            .configure("empty", &capture, 0, id)
            .unwrap_err()
            .code(),
        ErrorCode::OperationUnsupported
    );
    assert_eq!(
        storage_engine::canonical_json(fixture.engine.state()).unwrap(),
        before
    );
    for (disk, expected) in fixture.config.disks.iter().zip(disk_bytes) {
        assert_eq!(files(&disk.root), expected);
        let projection = disk
            .root
            .join("histories")
            .join(&fixture.repository.history_tree_id)
            .join("directory-backups")
            .join(id.to_string());
        assert!(!projection.exists());
    }
}

#[test]
fn directory_publication_does_not_invent_extra_file_copies_or_flatten_prefixes() {
    let mut fixture = Fixture::new(false);
    fixture.save("docs/report", b"real body");
    mkdir(&mut fixture, "docs/empty");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let before_versions = fixture.engine.state().versions.len();
    let file_receipt = fixture.configure("docs", &capture, 0, operation()).unwrap();
    assert_eq!(
        file_receipt.protection_at_publish,
        ProtectionAtPublish::TargetMet
    );
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let receipt = fixture
        .configure("docs/empty", &capture, 0, operation())
        .unwrap();
    assert_eq!(fixture.engine.state().versions.len(), before_versions);
    assert_eq!(
        receipt.protection_at_publish,
        ProtectionAtPublish::NotApplicable
    );
    assert_eq!(
        names(&fixture, "C01"),
        ["disk-1/disk-0/Repository/docs/empty",]
    );
    let old_capture = fixture.engine.recursive_backup_capture().unwrap();
    mkdir(&mut fixture, "later");
    let before = storage_engine::canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .configure("docs/empty", &old_capture, 0, operation())
            .unwrap_err()
            .code(),
        ErrorCode::ScopeChanged
    );
    assert_eq!(
        storage_engine::canonical_json(fixture.engine.state()).unwrap(),
        before
    );
}

#[test]
fn missing_metadata_member_cold_open_does_not_repair_directory_projections() {
    let mut fixture = Fixture::new(false);
    mkdir(&mut fixture, "empty");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let receipt = fixture
        .configure("empty", &capture, 0, operation())
        .unwrap();
    let projections = [
        physical(&fixture, 1, receipt.operation_id, "disk-0/Repository/empty"),
        physical(
            &fixture,
            2,
            receipt.operation_id,
            "disk-1/disk-0/Repository/empty",
        ),
    ];
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    for path in &projections {
        fs::remove_dir(path).unwrap();
    }
    let offline = fixture.temp.path().join("offline-primary");
    fs::rename(&fixture.config.disks[0].root, &offline).unwrap();
    let before = [
        files(&fixture.config.disks[1].root),
        files(&fixture.config.disks[2].root),
    ];
    let reopened = Engine::open(&system).unwrap();
    assert!(reopened.is_read_only());
    assert_eq!(reopened.operation(receipt.operation_id).unwrap(), receipt);
    for (index, path) in projections.iter().enumerate() {
        assert!(
            !path.exists(),
            "read-only cold open repaired a directory: {}",
            path.display()
        );
        assert_eq!(files(&fixture.config.disks[index + 1].root), before[index]);
    }
    drop(reopened);
    fs::rename(&offline, &fixture.config.disks[0].root).unwrap();
    let writable = Engine::open(&system).unwrap();
    assert!(!writable.is_read_only());
    assert!(projections.iter().all(|path| path.is_dir()));
}

#[test]
fn membership_fenced_cold_open_does_not_repair_directory_projections() {
    let mut fixture = Fixture::new(false);
    mkdir(&mut fixture, "empty");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let receipt = fixture
        .configure("empty", &capture, 0, operation())
        .unwrap();
    let projections = [
        physical(&fixture, 1, receipt.operation_id, "disk-0/Repository/empty"),
        physical(
            &fixture,
            2,
            receipt.operation_id,
            "disk-1/disk-0/Repository/empty",
        ),
    ];
    let fence = fixture
        .engine
        .begin_disk_exit(
            &fixture.config.disks[2].disk_id,
            MembershipExitAction::PlannedOffline,
            "admin",
            operation(),
        )
        .unwrap();
    assert!(fence.all_members_fenced());
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    for path in &projections {
        fs::remove_dir(path).unwrap();
    }
    let before = fixture
        .config
        .disks
        .iter()
        .map(|disk| files(&disk.root))
        .collect::<Vec<_>>();
    for preserve_pending in [false, true] {
        let reopened = if preserve_pending {
            Engine::open_for_recovery(&system)
        } else {
            Engine::open(&system)
        }
        .unwrap();
        assert!(reopened.is_read_only());
        assert_eq!(reopened.membership_fence_status(), Some(fence.clone()));
        assert_eq!(reopened.operation(receipt.operation_id).unwrap(), receipt);
        assert!(
            projections.iter().all(|path| !path.exists()),
            "fenced open recreated a physical directory"
        );
        for (disk, expected) in fixture.config.disks.iter().zip(&before) {
            assert_eq!(&files(&disk.root), expected);
        }
    }
}
