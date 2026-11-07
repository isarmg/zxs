//! Empty snapshot following uses actual native publications and cold disk reads.
use super::*;

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
fn rename(fixture: &mut Fixture, from: &str, to: &str, id: OperationId) -> Result<OperationResult> {
    fixture.engine.file_namespace(
        &fixture.repository.repository_id,
        NamespaceOperation::parse(NamespaceOperationKind::Rename, from, Some(to)).unwrap(),
        "admin",
        id,
    )
}
fn names(fixture: &Fixture, slot: &str) -> Vec<String> {
    let mut paths = fixture
        .engine
        .published_backup_directories(slot)
        .unwrap()
        .iter()
        .map(|row| row.path().as_str().to_owned())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}
fn projection(fixture: &Fixture, disk: usize, id: OperationId) -> std::path::PathBuf {
    fixture.config.disks[disk]
        .root
        .join("histories")
        .join(&fixture.repository.history_tree_id)
        .join("directory-backups")
        .join(id.to_string())
}

#[test]
fn empty_follow_updates_captured_rows_and_later_files_without_rewriting_receipts() {
    let mut fixture = Fixture::new(false);
    mkdir(&mut fixture, "empty");
    mkdir(&mut fixture, "empty/child");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let configured = fixture
        .configure("empty", &capture, 0, operation())
        .unwrap();
    let original_bytes = files(&projection(&fixture, 2, configured.operation_id));
    mkdir(&mut fixture, "empty/new-not-captured");
    let file = fixture.save("empty/child/report", b"after empty configuration");
    let renamed = rename(&mut fixture, "empty", "renamed", operation()).unwrap();
    assert_eq!(renamed.kind, "file.namespace.recursive-directories");
    assert_eq!(
        names(&fixture, "C01"),
        [
            "disk-1/disk-0/Repository/renamed",
            "disk-1/disk-0/Repository/renamed/child"
        ]
    );
    fixture.assert_body_paths(&file, "renamed/child/report", b"after empty configuration");
    assert_eq!(
        files(&projection(&fixture, 2, configured.operation_id)),
        original_bytes
    );
    assert!(
        projection(&fixture, 2, renamed.operation_id)
            .join("namespace/disk-1/disk-0/Repository/renamed/child")
            .is_dir()
    );
    let old_generation = fixture.engine.state().protection_policies
        [configured.result["policy"]["policy_id"].as_str().unwrap()]
    .generation;
    let nested = rename(&mut fixture, "renamed/child", "renamed/final", operation()).unwrap();
    assert_eq!(
        fixture.engine.state().protection_policies
            [configured.result["policy"]["policy_id"].as_str().unwrap()]
        .generation,
        old_generation + 1
    );
    let missing = projection(&fixture, 2, nested.operation_id)
        .join("namespace/disk-1/disk-0/Repository/renamed/final");
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fs::remove_dir(&missing).unwrap();
    fixture.engine = Engine::open(&system).unwrap();
    assert!(missing.is_dir());
    assert_eq!(
        names(&fixture, "C01"),
        [
            "disk-1/disk-0/Repository/renamed",
            "disk-1/disk-0/Repository/renamed/final"
        ]
    );
    fixture.assert_body_paths(&file, "renamed/final/report", b"after empty configuration");
    assert_eq!(
        fixture.engine.operation(configured.operation_id).unwrap(),
        configured
    );
    assert_eq!(
        rename(&mut fixture, "empty", "renamed", renamed.operation_id).unwrap(),
        renamed
    );
    let scope = configured.result["scope"]["id"].as_str().unwrap();
    assert_eq!(
        fixture
            .engine
            .configure_recursive_backups(RecursiveBackupConfigurationRequest {
                repository_id: &fixture.repository.repository_id,
                path: "empty",
                expected_scope_id: scope,
                target_copies: 3,
                expected_generation: 0,
                capture: &capture,
                principal: "admin",
                operation_id: configured.operation_id,
            })
            .unwrap(),
        configured
    );
}

#[test]
fn empty_follow_keeps_independent_child_policies_and_one_record_per_target() {
    let mut fixture = Fixture::new(true);
    mkdir(&mut fixture, "outer");
    mkdir(&mut fixture, "outer/inner");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let scope = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, "outer/inner")
        .unwrap()
        .scope;
    let child = fixture
        .engine
        .configure_recursive_backups(RecursiveBackupConfigurationRequest {
            repository_id: &fixture.repository.repository_id,
            path: "outer/inner",
            expected_scope_id: &scope.id,
            target_copies: 2,
            expected_generation: 0,
            capture: &capture,
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let parent = fixture
        .configure("outer", &capture, 0, operation())
        .unwrap();
    let moved = rename(&mut fixture, "outer", "moved", operation()).unwrap();
    assert_eq!(
        moved.result["recursive_directory_follow"]["policies"]
            .as_object()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        names(&fixture, "B01"),
        ["disk-0/moved", "disk-0/moved/inner"]
    );
    assert_eq!(names(&fixture, "C01"), ["disk-1/disk-0/moved"]);
    assert_eq!(
        fs::read(projection(&fixture, 1, moved.operation_id).join("record.json")).unwrap(),
        fs::read(projection(&fixture, 2, moved.operation_id).join("record.json")).unwrap()
    );
    rename(&mut fixture, "moved/inner", "elsewhere/inner", operation()).unwrap();
    assert_eq!(
        names(&fixture, "B01"),
        ["disk-0/elsewhere/inner", "disk-0/moved"]
    );
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(
        names(&fixture, "B01"),
        ["disk-0/elsewhere/inner", "disk-0/moved"]
    );
    assert_eq!(
        fixture.engine.operation(parent.operation_id).unwrap(),
        parent
    );
    assert_eq!(fixture.engine.operation(child.operation_id).unwrap(), child);
}

#[test]
fn empty_follow_actual_batch_move_after_shared_layout_activation() {
    use storage_engine::{
        DirectoryBatchKind, DirectoryNamespaceBatch, DirectoryNamespaceItem,
        DirectoryNamespaceSource, DirectoryNamespaceTarget,
        directory_git::DirectoryGitInitializeBody,
    };
    let mut fixture = Fixture::new(true);
    mkdir(&mut fixture, "empty");
    mkdir(&mut fixture, "destination");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let configured = fixture
        .configure("empty", &capture, 0, operation())
        .unwrap();
    let head = fixture.engine.state().branches[&fixture.repository.main_branch_id]
        .head_commit_id
        .clone();
    fixture
        .engine
        .initialize_directory_git(
            &fixture.repository.repository_id,
            &DirectoryGitInitializeBody {
                path: "".into(),
                expected_namespace_head: head,
                operation_id: operation(),
            },
            "admin",
        )
        .unwrap();
    assert!(
        fixture
            .engine
            .reconcile_directory_layout(&fixture.repository.repository_id)
            .unwrap()
            .directories
            > 0
    );
    let status = fixture
        .engine
        .directory_git_status(&fixture.repository.repository_id, "")
        .unwrap();
    let branch = fixture.engine.state().directory_git_repositories[&status.directory_id]
        .main_branch_id
        .clone();
    let tree = &fixture.engine.state().directory_branches[&branch];
    let source = tree
        .entries
        .iter()
        .find(|entry| entry.path() == "empty")
        .unwrap();
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Move,
        items: vec![DirectoryNamespaceItem {
            source: DirectoryNamespaceSource {
                directory_branch_id: branch.clone(),
                expected_tree_id: tree.head_tree_id.clone(),
                path: "empty".into(),
                entry_id: source.entry_id.clone(),
            },
            target: Some(DirectoryNamespaceTarget {
                directory_branch_id: branch.clone(),
                expected_tree_id: tree.head_tree_id.clone(),
                path: "destination/empty".into(),
            }),
        }],
    };
    let id = operation();
    let receipt = fixture
        .engine
        .directory_namespace_batch(&fixture.repository.repository_id, &request, "admin", id)
        .unwrap();
    assert_eq!(
        receipt.kind,
        "directory.namespace.batch.recursive-directories"
    );
    assert_eq!(names(&fixture, "C01"), ["disk-1/disk-0/destination/empty"]);
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(
        fixture
            .engine
            .directory_namespace_batch(&fixture.repository.repository_id, &request, "admin", id)
            .unwrap(),
        receipt
    );
    assert_eq!(names(&fixture, "C01"), ["disk-1/disk-0/destination/empty"]);
    assert_eq!(
        fixture.engine.operation(configured.operation_id).unwrap(),
        configured
    );
}

#[test]
fn empty_follow_post_decision_failure_recovers_new_projection_only() {
    let mut fixture = Fixture::new(true);
    mkdir(&mut fixture, "empty");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let old = fixture
        .configure("empty", &capture, 0, operation())
        .unwrap();
    let old_bytes = files(&projection(&fixture, 1, old.operation_id));
    let id = operation();
    let blocked = projection(&fixture, 1, id).join("namespace");
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(blocked.parent().unwrap())
        .unwrap();
    fs::write(&blocked, b"test-owned obstruction").unwrap();
    assert_eq!(
        rename(&mut fixture, "empty", "moved", id)
            .unwrap_err()
            .code(),
        ErrorCode::InDoubt
    );
    assert_eq!(fs::read(&blocked).unwrap(), b"test-owned obstruction");
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fs::remove_file(&blocked).unwrap();
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(names(&fixture, "B01"), ["disk-0/moved"]);
    let receipt = fixture.engine.operation(id).unwrap();
    assert_eq!(rename(&mut fixture, "empty", "moved", id).unwrap(), receipt);
    assert_eq!(files(&projection(&fixture, 1, old.operation_id)), old_bytes);
    assert_eq!(fixture.engine.operation(old.operation_id).unwrap(), old);
}

#[test]
fn empty_follow_stopped_target_rejects_without_new_projection() {
    let mut fixture = Fixture::new(true);
    mkdir(&mut fixture, "empty");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    fixture
        .configure("empty", &capture, 0, operation())
        .unwrap();
    fixture.exit(1, MembershipExitAction::PlannedOffline);
    let previous = storage_engine::canonical_json(fixture.engine.state()).unwrap();
    let frozen = files(&fixture.config.disks[1].root);
    let id = operation();
    let error = rename(&mut fixture, "empty", "moved", id).unwrap_err();
    assert!(matches!(
        error.code(),
        ErrorCode::PlacementInvalid | ErrorCode::SlotNotFound
    ));
    assert_eq!(
        storage_engine::canonical_json(fixture.engine.state()).unwrap(),
        previous
    );
    assert_eq!(files(&fixture.config.disks[1].root), frozen);
    assert!(!projection(&fixture, 1, id).exists());
}

#[test]
fn empty_follow_custom_replacement_does_not_keep_old_directory_authority() {
    let mut fixture = Fixture::new(true);
    mkdir(&mut fixture, "empty");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let receipt = fixture
        .configure("empty", &capture, 0, operation())
        .unwrap();
    let file = fixture.save("empty/report", b"custom replacement");
    let policy = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, "empty")
        .unwrap();
    let prefixes = BTreeMap::from([
        ("B01".into(), "custom-one".into()),
        ("C01".into(), "custom-two".into()),
    ]);
    fixture
        .engine
        .configure_backups(storage_engine::BackupConfigurationRequest {
            repository_id: &fixture.repository.repository_id,
            path: "empty",
            target_copies: 3,
            backup_prefixes: &prefixes,
            expected_generation: policy.generation,
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    let paths = fixture.engine.state().file_backup_paths[&file.file_id].clone();
    assert!(names(&fixture, "B01").is_empty());
    let renamed = rename(&mut fixture, "empty", "moved", operation()).unwrap();
    assert_eq!(renamed.kind, "file.namespace");
    assert_eq!(
        fixture.engine.state().file_backup_paths[&file.file_id],
        paths
    );
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert!(names(&fixture, "C01").is_empty());
    assert_eq!(
        fixture.engine.state().file_backup_paths[&file.file_id],
        paths
    );
    assert_eq!(
        fixture.engine.operation(receipt.operation_id).unwrap(),
        receipt
    );
}

#[test]
fn empty_follow_overlapping_receipts_use_distinct_listing_and_projection_budgets() {
    // Adapted from the independent Codex audit's 9 rows / 5 IDs / 8 nodes witness.
    let temp = tempfile::Builder::new()
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .tempdir()
        .unwrap();
    let mut config =
        Config::development((0..4).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    config.max_entries = 8;
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
    for path in ["a", "a/b", "a/b/c", "a/b/c/d", "a/b/c/d/e"] {
        mkdir(&mut fixture, path);
    }
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let parent = fixture.configure("a", &capture, 0, operation()).unwrap();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let child = fixture.configure("a/b", &capture, 0, operation()).unwrap();
    assert_eq!(names(&fixture, "C01").len(), 5);
    for row in fixture.engine.published_backup_directories("C01").unwrap() {
        assert_eq!(
            row.operation_id(),
            if row.path().as_str() == "disk-1/disk-0/Repository/a" {
                parent.operation_id
            } else {
                child.operation_id
            }
        );
    }
    // A new follow receipt contains both snapshots. Its physical prepare budget
    // also counts distinct rows instead of rejecting nine repeated raw rows.
    let moved = rename(&mut fixture, "a", "renamed", operation()).unwrap();
    assert_eq!(names(&fixture, "C01").len(), 5);
    assert!(
        projection(&fixture, 2, moved.operation_id)
            .join("namespace/disk-1/disk-0/Repository/renamed/b/c/d/e")
            .is_dir()
    );
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(names(&fixture, "C01").len(), 5);
    assert_eq!(
        fixture.engine.operation(parent.operation_id).unwrap(),
        parent
    );
    assert_eq!(fixture.engine.operation(child.operation_id).unwrap(), child);
}
