//! Effective modes come from public admitted reads, not operation-kind guesses.
use super::*;
use storage_contract::BackupConfigurationMode;
use storage_engine::BackupConfigurationRequest;

fn mode(fixture: &Fixture, path: &str) -> BackupConfigurationMode {
    fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, path)
        .unwrap()
        .effective_mode
}

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

fn rename(fixture: &mut Fixture, from: &str, to: &str) {
    fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, from, Some(to)).unwrap(),
            "admin",
            operation(),
        )
        .unwrap();
}

fn tree(fixture: &mut Fixture, path: &str) -> OperationResult {
    let selected = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, path)
        .unwrap();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    fixture
        .engine
        .configure_recursive_backup_tree(RecursiveBackupConfigurationRequest {
            repository_id: &fixture.repository.repository_id,
            path,
            expected_scope_id: &selected.scope.id,
            target_copies: 3,
            expected_generation: selected.generation,
            capture: &capture,
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap()
}

#[test]
fn effective_tree_mode_survives_inheritance_follow_and_cold_then_files_replacement() {
    let mut fixture = Fixture::new(false);
    let original = fixture.save("docs/report", b"original");
    mkdir(&mut fixture, "docs/empty");
    assert_eq!(
        mode(&fixture, "docs"),
        BackupConfigurationMode::Unconfigured
    );
    assert_eq!(
        mode(&fixture, "docs/report"),
        BackupConfigurationMode::Unconfigured
    );

    let published = tree(&mut fixture, "docs");
    for path in ["docs", "docs/empty", "docs/report"] {
        assert_eq!(
            mode(&fixture, path),
            BackupConfigurationMode::RecursiveDirectoryTree
        );
    }
    mkdir(&mut fixture, "docs/later");
    let later_id = fixture
        .engine
        .namespace_tree(&fixture.repository.main_branch_id)
        .unwrap()
        .get("docs/later")
        .unwrap()
        .entry_id
        .clone();
    for slot in ["B01", "C01"] {
        assert!(
            !fixture
                .engine
                .published_backup_directories(slot)
                .unwrap()
                .iter()
                .any(|row| row.directory_id() == later_id),
            "effective Tree mode is not evidence that a later directory was captured"
        );
    }
    let inherited = fixture.save("docs/later/new", b"inherited");
    for path in ["docs/later", "docs/later/new"] {
        assert_eq!(
            mode(&fixture, path),
            BackupConfigurationMode::RecursiveDirectoryTree
        );
    }
    rename(&mut fixture, "docs", "renamed");
    for path in [
        "renamed",
        "renamed/empty",
        "renamed/report",
        "renamed/later/new",
    ] {
        assert_eq!(
            mode(&fixture, path),
            BackupConfigurationMode::RecursiveDirectoryTree
        );
    }
    fixture.assert_body_paths(&original, "renamed/report", b"original");
    fixture.assert_body_paths(&inherited, "renamed/later/new", b"inherited");
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    for path in [
        "renamed",
        "renamed/empty",
        "renamed/report",
        "renamed/later/new",
    ] {
        assert_eq!(
            mode(&fixture, path),
            BackupConfigurationMode::RecursiveDirectoryTree
        );
    }
    assert_eq!(
        fixture.engine.operation(published.operation_id).unwrap(),
        published
    );

    let selected = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, "renamed")
        .unwrap();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let replacement = fixture
        .configure("renamed", &capture, selected.generation, operation())
        .unwrap();
    for path in [
        "renamed",
        "renamed/empty",
        "renamed/report",
        "renamed/later/new",
    ] {
        assert_eq!(
            mode(&fixture, path),
            BackupConfigurationMode::RecursiveFiles
        );
    }
    assert_eq!(
        fixture.engine.operation(published.operation_id).unwrap(),
        published
    );
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(
        mode(&fixture, "renamed"),
        BackupConfigurationMode::RecursiveFiles
    );
    assert_eq!(
        mode(&fixture, "renamed/later/new"),
        BackupConfigurationMode::RecursiveFiles
    );
    assert_eq!(
        fixture.engine.operation(replacement.operation_id).unwrap(),
        replacement
    );
    assert_eq!(
        fixture.engine.operation(published.operation_id).unwrap(),
        published
    );
}

#[test]
fn effective_custom_mode_is_not_inferred_from_recursive_looking_prefixes() {
    let mut fixture = Fixture::new(false);
    fixture.save("docs/report", b"custom");
    let selected = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, "docs")
        .unwrap();
    let prefixes = BTreeMap::from([
        ("B01".into(), "disk-0/Repository/docs".into()),
        ("C01".into(), "disk-1/disk-0/Repository/docs".into()),
    ]);
    let receipt = fixture
        .engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &fixture.repository.repository_id,
            path: "docs",
            target_copies: 3,
            backup_prefixes: &prefixes,
            expected_generation: selected.generation,
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    assert_eq!(mode(&fixture, "docs"), BackupConfigurationMode::Custom);
    assert_eq!(
        mode(&fixture, "docs/report"),
        BackupConfigurationMode::Custom
    );
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(mode(&fixture, "docs"), BackupConfigurationMode::Custom);
    assert_eq!(
        mode(&fixture, "docs/report"),
        BackupConfigurationMode::Custom
    );
    assert_eq!(
        fixture.engine.operation(receipt.operation_id).unwrap(),
        receipt
    );
}

#[test]
fn detached_file_reads_retained_mode_without_rewriting_original_tree_receipt() {
    let mut fixture = Fixture::new(false);
    let file = fixture.save("docs/report", b"retained");
    let original = tree(&mut fixture, "docs");
    rename(&mut fixture, "docs/report", "outside/report");
    assert_eq!(
        mode(&fixture, "outside/report"),
        BackupConfigurationMode::RecursiveDirectoryTree
    );
    let replacement = tree(&mut fixture, "docs");
    let policy = &fixture.engine.state().protection_policies
        [&fixture.engine.state().file_backup_policies[&file.file_id]];
    assert!(
        !policy.inheritable,
        "explicit reconfiguration must detach the moved file"
    );
    assert_eq!(
        mode(&fixture, "outside/report"),
        BackupConfigurationMode::Retained
    );
    assert_eq!(
        mode(&fixture, "docs"),
        BackupConfigurationMode::RecursiveDirectoryTree
    );
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(
        mode(&fixture, "outside/report"),
        BackupConfigurationMode::Retained
    );
    assert_eq!(
        mode(&fixture, "docs"),
        BackupConfigurationMode::RecursiveDirectoryTree
    );
    assert_eq!(
        fixture.engine.operation(original.operation_id).unwrap(),
        original
    );
    assert_eq!(
        fixture.engine.operation(replacement.operation_id).unwrap(),
        replacement
    );
    fixture.assert_body_paths(&file, "outside/report", b"retained");
}

#[test]
fn initial_custom_policy_reads_unverified_before_and_after_cold_open() {
    let mut fixture = Fixture::initial(false, 4);
    assert!(fixture.engine.current_disk_registry_commitment().is_none());
    fixture.save("docs/report", b"initial custom");
    assert_eq!(
        mode(&fixture, "docs"),
        BackupConfigurationMode::Unconfigured
    );
    let selected = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, "docs")
        .unwrap();
    let prefixes = BTreeMap::from([("B01".into(), "archive/docs".into())]);
    let receipt = fixture
        .engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &fixture.repository.repository_id,
            path: "docs",
            target_copies: 2,
            backup_prefixes: &prefixes,
            expected_generation: selected.generation,
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    for path in ["docs", "docs/report"] {
        assert_eq!(mode(&fixture, path), BackupConfigurationMode::Unverified);
    }
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert!(fixture.engine.current_disk_registry_commitment().is_none());
    for path in ["docs", "docs/report"] {
        assert_eq!(mode(&fixture, path), BackupConfigurationMode::Unverified);
    }
    assert_eq!(
        fixture.engine.operation(receipt.operation_id).unwrap(),
        receipt
    );
}
