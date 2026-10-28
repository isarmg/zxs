use std::process::Command;
#[test]
fn rescue_exposes_only_offline_recovery_and_configuration_operations() {
    let binary = env!("CARGO_BIN_EXE_zxs-rescue");
    for name in [
        "restore-system",
        "recovery-preview",
        "recovery-commit",
        "offline-check",
        "scrub",
        "offline-export",
        "package",
        "configuration",
    ] {
        assert!(
            Command::new(binary)
                .args([name, "--help"])
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    for name in [
        "init",
        "init-auth",
        "serve",
        "account",
        "create-repo",
        "export-history",
        "register-disk",
        "config-template",
        "--server",
    ] {
        let output = Command::new(binary).arg(name).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
}
#[test]
fn recovery_requires_explicit_loss_acknowledgement_before_opening_pool() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-rescue"))
        .current_dir(directory.path())
        .args(["recovery-commit", "recovery", "--digest", "digest"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--accept-permanent-loss"));
    assert!(!directory.path().join("zxs-data").exists());
}

#[test]
fn recovery_commands_report_returned_engine_seal_failure_once_before_command_rejection() {
    use storage_engine::{CommitState, Config, Engine, OperationId, RepositoryName};

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("system");
    let disks = ["a", "b", "c"].map(|name| directory.path().join(name));
    let mut engine = Engine::initialize(&root, Config::development(disks.clone())).unwrap();
    let operation = OperationId::generate();
    engine
        .create_repository(
            &RepositoryName::parse("unsealed").unwrap(),
            "P01",
            1,
            "admin",
            operation,
        )
        .unwrap();
    drop(engine);
    let seals: Vec<_> = std::fs::read_dir(disks[0].join("metadata/publication-seals"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(seals.len(), 1);
    let candidate = seals[0].file_stem().unwrap().to_str().unwrap();
    let seal_name = seals[0].file_name().unwrap();
    std::fs::rename(&disks[2], directory.path().join("offline")).unwrap();
    let journals: Vec<_> = disks[..2]
        .iter()
        .map(|disk| std::fs::read(disk.join("metadata/application-log/journal.bin")).unwrap())
        .collect();
    for disk in &disks[..2] {
        std::fs::remove_file(disk.join("metadata/publication-seals").join(seal_name)).unwrap();
    }
    for args in [
        vec![
            "recovery-preview".to_owned(),
            operation.to_string(),
            "--reason".into(),
            "inspect missing barrier".into(),
        ],
        vec![
            "recovery-commit".to_owned(),
            storage_engine::new_id("recovery"),
            "--digest".into(),
            "unused".into(),
            "--accept-permanent-loss".into(),
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_zxs-rescue"))
            .arg("--root")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            error.matches("recovery seal verification failed").count(),
            1,
            "{error}"
        );
        assert!(error.contains("Publication"), "{error}");
        assert!(error.contains(candidate), "{error}");
        assert!(error.contains("Filesystem(Io("), "{error}");
        assert!(error.contains("NotFound"), "{error}");
        assert!(error.contains("zxs-rescue:"), "{error}");
        let engine = Engine::open_for_recovery(&root).unwrap();
        assert!(engine.is_read_only());
        assert!(engine.state().repositories.is_empty());
        assert_eq!(
            engine.operation(operation).unwrap().commit_state,
            CommitState::InDoubt
        );
        for (disk, expected) in disks[..2].iter().zip(&journals) {
            assert_eq!(
                std::fs::read(disk.join("metadata/application-log/journal.bin")).unwrap(),
                *expected
            );
        }
    }
}

#[test]
fn package_restore_rejects_zero_copies_before_opening_pool_or_package() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-rescue"))
        .current_dir(directory.path())
        .args([
            "package",
            "restore",
            "missing-package",
            "--primary-slot",
            "P01",
            "--copies",
            "0",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("--copies"), "{error}");
    assert!(!error.contains("IO_ERROR"), "{error}");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn rescue_restores_from_two_old_members_without_management_commands() {
    use storage_server::{auth::AuthStore, engine_backend::EngineBackend};
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("original");
    let replacement = directory.path().join("replacement");
    let disks = ["a", "b", "c"].map(|slot| directory.path().join(slot));
    let mut engine = storage_engine::Engine::initialize(
        &original,
        storage_engine::Config::development(disks.clone()),
    )
    .unwrap();
    let baseline = engine.config().clone();
    let repository = engine
        .create_repository(
            &storage_engine::RepositoryName::parse("rescue-fixture").unwrap(),
            "P01",
            3,
            "admin",
            storage_engine::OperationId::generate(),
        )
        .unwrap();
    let backend = std::sync::Arc::new(EngineBackend::new(engine));
    backend
        .protect_auth(AuthStore::initialize(&original, "admin", "test-only-password").unwrap())
        .unwrap();
    drop(backend);
    std::fs::remove_dir_all(&original).unwrap();
    let limits = |records: usize| {
        [
            "--max-retained-workspaces".to_owned(),
            records.to_string(),
            "--max-retained-workspaces-per-principal".to_owned(),
            baseline.max_retained_workspaces_per_principal.to_string(),
            "--max-retained-workspace-metadata-bytes".to_owned(),
            baseline.max_retained_workspace_metadata_bytes.to_string(),
            "--max-retained-workspace-metadata-bytes-per-principal".to_owned(),
            baseline
                .max_retained_workspace_metadata_bytes_per_principal
                .to_string(),
        ]
    };
    let rejected = directory.path().join("rejected");
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-rescue"))
        .arg("--root")
        .arg(&rejected)
        .args(["restore-system", "--disks"])
        .args(&disks[..2])
        .args(limits(baseline.max_retained_workspaces - 1))
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("CONFIG_INVALID"));
    assert!(!rejected.join("config.json").exists());
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-rescue"))
        .arg("--root")
        .arg(&replacement)
        .args(["restore-system", "--disks"])
        .args(&disks[..2])
        .args(limits(baseline.max_retained_workspaces + 1))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let receipt: json_codec::Value = json_codec::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["credential_recovery_snapshot_available"], true);
    let restored = storage_engine::Engine::open(&replacement).unwrap();
    assert!(
        restored
            .state()
            .repositories
            .contains_key(&repository.repository_id)
    );
    assert!(restored.auth_state().is_some());
    assert_eq!(
        json_codec::to_value(restored.config()).unwrap(),
        json_codec::to_value(&baseline).unwrap()
    );
}

#[test]
fn history_package_round_trip_preserves_checks_and_refuses_overwrite() {
    use std::os::unix::fs::PermissionsExt;
    use storage_engine::{CommitRequest, Config, Engine};
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    let initialize = |root: &std::path::Path, prefix: &str| {
        let disks = ["a", "b", "c"].map(|slot| directory.path().join(format!("{prefix}-{slot}")));
        Engine::initialize(root, Config::development(disks)).unwrap()
    };
    let mut engine = initialize(&source, "source");
    let repository = engine
        .create_repository(
            &storage_engine::RepositoryName::parse("package-fixture").unwrap(),
            "P01",
            3,
            "admin",
            storage_engine::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "verified.txt",
            b"verified package bytes",
        )
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_engine::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "package fixture".into(),
            request_digest: None,
        })
        .unwrap();
    drop(engine);
    let package = directory.path().join("package");
    let invoke = |root: &std::path::Path, args: &[&std::ffi::OsStr]| {
        Command::new(env!("CARGO_BIN_EXE_zxs-rescue"))
            .arg("--root")
            .arg(root)
            .arg("package")
            .args(args)
            .output()
            .unwrap()
    };
    let export = [
        "export".as_ref(),
        repository.history_tree_id.as_ref(),
        package.as_os_str(),
    ];
    let output = invoke(&source, &export);
    assert!(output.status.success(), "{output:?}");
    assert!(
        !invoke(&source, &export).status.success(),
        "must never overwrite package destination"
    );
    assert!(
        invoke(&source, &["check".as_ref(), package.as_os_str()])
            .status
            .success()
    );
    drop(initialize(&target, "target"));
    let output = invoke(
        &target,
        &[
            "restore".as_ref(),
            package.as_os_str(),
            "--primary-slot".as_ref(),
            "P01".as_ref(),
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let restored = Engine::open(&target).unwrap();
    assert!(
        restored
            .state()
            .repositories
            .contains_key(&repository.repository_id)
    );
    let entries = restored.list_tree(&repository.main_branch_id).unwrap();
    assert_eq!(entries.len(), 1);
    let mut content = restored.open_content(&entries[0].version_id).unwrap();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut content.file, &mut bytes).unwrap();
    assert_eq!(bytes, b"verified package bytes");
    drop(content);
    drop(restored);
    std::fs::write(package.join("publication.json"), b"invalid proof").unwrap();
    assert!(
        !invoke(&source, &["check".as_ref(), package.as_os_str()])
            .status
            .success()
    );
}

#[test]
fn configuration_commands_edit_only_an_uninitialized_draft() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path();
    let output = directory.join("custom.json");
    let disks = [
        directory.join("p"),
        directory.join("b"),
        directory.join("c"),
    ];
    let binary = env!("CARGO_BIN_EXE_zxs-rescue");
    let result = Command::new(binary)
        .args(["configuration", "template", "--disks"])
        .args(&disks)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let original = std::fs::read(&output).unwrap();
    let additional = directory.join("fourth");
    let result = Command::new(binary)
        .args(["configuration", "register-disk", "--config"])
        .arg(&output)
        .args(["--slot", "P02", "--disk-id"])
        .arg(storage_engine::new_id("disk"))
        .arg("--path")
        .arg(&additional)
        .args(["--level", "1"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let config: storage_engine::Config =
        storage_engine::decode_strict(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(config.disks.len(), 4);
    assert_eq!(
        std::fs::read(directory.join("custom.json.before-registration")).unwrap(),
        original
    );
    assert!(config.disks.iter().all(|disk| !disk.root.exists()));
    assert!(!directory.join("auth.json").exists());
    let result = Command::new(binary)
        .args(["configuration", "template", "--disks"])
        .args(&disks)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert!(!result.status.success());
}

#[test]
fn template_cli_accepts_one_disk_and_rejects_no_disks() {
    let directory = tempfile::tempdir().unwrap();
    let disk = directory.path().join("only");
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-rescue"))
        .args(["configuration", "template", "--disks"])
        .arg(&disk)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config: storage_engine::Config = storage_engine::decode_strict(&output.stdout).unwrap();
    assert_eq!(config.disks.len(), 1);
    assert_eq!(config.disks[0].level, 1);
    assert!(!disk.exists());
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-rescue"))
        .args(["configuration", "template", "--disks"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn invalid_retained_workspace_flags_do_not_create_state() {
    for flags in [
        vec!["--max-retained-workspaces", "2"],
        vec![
            "--max-retained-workspaces",
            "0",
            "--max-retained-workspaces-per-principal",
            "1",
            "--max-retained-workspace-metadata-bytes",
            "1",
            "--max-retained-workspace-metadata-bytes-per-principal",
            "1",
        ],
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("pool");
        let output = Command::new(env!("CARGO_BIN_EXE_zxs-rescue"))
            .arg("--root")
            .arg(&root)
            .args(["package", "export", "history"])
            .arg(directory.path().join("package"))
            .args(flags)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("TOKIO_WORKER_THREADS"));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[test]
fn malformed_operation_id_is_rejected_before_opening_the_pool() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("unopened-pool");
    for args in [
        vec!["recovery-preview", "op_BAD", "--reason", "review"],
        vec![
            "package",
            "restore",
            "/unused-package",
            "--primary-slot",
            "P01",
            "--operation-id",
            "op_BAD",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_zxs-rescue"))
            .arg("--root")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("invalid op identifier")
        );
        assert!(!root.exists());
    }
}

#[test]
fn scrub_is_a_real_exclusive_offline_consumer_with_explicit_partial_failure() {
    use storage_engine::{Config, Engine, OperationId, RepositoryName};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("system");
    let disks = ["a", "b", "c"].map(|name| directory.path().join(name));
    let mut engine = Engine::initialize(&root, Config::development(disks.clone())).unwrap();
    engine
        .create_repository(
            &RepositoryName::parse("scrub-fixture").unwrap(),
            "P01",
            3,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_zxs-rescue"))
            .arg("--root")
            .arg(&root)
            .arg("scrub")
            .output()
            .unwrap()
    };
    let locked = invoke();
    assert_eq!(locked.status.code(), Some(1));
    assert!(locked.stdout.is_empty());
    assert!(String::from_utf8_lossy(&locked.stderr).contains("WRITER_ALREADY_ACTIVE"));
    drop(engine);
    let healthy = invoke();
    assert!(healthy.status.success(), "{healthy:?}");
    let report: json_codec::Value = json_codec::from_slice(&healthy.stdout).unwrap();
    assert_eq!(report["state"], "complete_success");
    assert_eq!(report["metadata_members_checked"], 3);
    assert!(report["metadata_publications_verified"].as_u64().unwrap() > 0);
    assert_eq!(report["last_complete_success_at"], report["finished_at"]);
    std::fs::remove_dir_all(&disks[2]).unwrap();
    let partial = invoke();
    assert_eq!(partial.status.code(), Some(1), "{partial:?}");
    let report: json_codec::Value = json_codec::from_slice(&partial.stdout).unwrap();
    assert_eq!(report["state"], "partial_failure");
    assert!(report["last_complete_success_at"].is_null());
    assert!(report["identity_failures"].as_u64().unwrap() > 0);
    assert_eq!(report["issues"][0]["area"], "identity");
    assert_eq!(report["issues"][0]["target"]["slot_id"], "C01");
}

#[test]
fn scrub_cannot_turn_a_failed_open_into_a_healthy_report() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("absent-system");
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-rescue"))
        .arg("--root")
        .arg(&root)
        .arg("scrub")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!root.exists());
}

#[test]
fn scrub_help_describes_offline_scan_without_opening_pool() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-rescue"))
        .current_dir(directory.path())
        .args(["scrub", "--help"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(
        help.contains(
            "Scan retained publication metadata and required full copies; stop ZXS first"
        )
    );
    assert!(help.contains("Usage: zxs-rescue scrub [OPTIONS]"));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}
