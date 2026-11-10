use std::process::Command;
#[path = "../server/service_namespace.rs"]
mod service_namespace;
const HOST_NAMESPACE_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../test/server/service_namespace.py"
);

#[test]
fn helper_exposes_only_implemented_offline_operations() {
    let binary = env!("CARGO_BIN_EXE_zxs-helper");
    for name in [
        "serve",
        "install",
        "deployment",
        "host",
        "initialization",
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
        "account",
        "create-repo",
        "export-history",
        "register-disk",
        "config-template",
        "planned-offline",
        "disk-offline",
        "--server",
    ] {
        let output = Command::new(binary).arg(name).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
}

#[test]
fn helper_identity_is_consistent_without_creating_runtime_state() {
    let directory = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_zxs-helper");
    let output = Command::new(binary)
        .current_dir(directory.path())
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("zxs-helper {}\n", env!("CARGO_PKG_VERSION"))
    );
    let output = Command::new(binary)
        .current_dir(directory.path())
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("Offline auxiliary tool"));
    assert!(help.contains("Usage: zxs-helper"));
    assert!(help.contains("Every helper operation holds the /zxs lifetime lease until exit."));
    assert!(help.contains(
        "Explicit handoff exits this helper before starting verified installed service units;"
    ));
    assert!(help.contains("unauthenticated maintenance Web console"));
    assert!(!help.contains("zxs-rescue"));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}
#[test]
fn recovery_requires_explicit_loss_acknowledgement_before_opening_pool() {
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "recovery_requires_explicit_loss_acknowledgement_before_opening_pool",
    ) {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
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
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "recovery_commands_report_returned_engine_seal_failure_once_before_command_rejection",
    ) {
        return;
    }
    use storage_engine::{CommitState, Config, Engine, OperationId, RepositoryName};

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("system");
    let disks = ["a", "b", "c"].map(|name| directory.path().join(name));
    let mut engine =
        Engine::initialize(&root, Config::development(disks.clone()).unwrap()).unwrap();
    let operation = OperationId::generate().unwrap();
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
            storage_engine::new_id("recovery").unwrap(),
            "--digest".into(),
            "unused".into(),
            "--accept-permanent-loss".into(),
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
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
        assert!(error.contains("zxs-helper:"), "{error}");
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
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
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
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "rescue_restores_from_two_old_members_without_management_commands",
    ) {
        return;
    }
    use storage_server::{auth::AuthStore, engine_backend::EngineBackend};
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("original");
    let replacement = directory.path().join("replacement");
    let disks = ["a", "b", "c"].map(|slot| directory.path().join(slot));
    let mut engine = storage_engine::Engine::initialize(
        &original,
        storage_engine::Config::development(disks.clone()).unwrap(),
    )
    .unwrap();
    let baseline = engine.config().clone();
    let repository = engine
        .create_repository(
            &storage_engine::RepositoryName::parse("rescue-fixture").unwrap(),
            "P01",
            3,
            "admin",
            storage_engine::OperationId::generate().unwrap(),
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
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
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
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
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
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "history_package_round_trip_preserves_checks_and_refuses_overwrite",
    ) {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    use storage_engine::{CommitRequest, Config, Engine};
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let source = directory.path().join("source");
    let target = directory.path().join("target");
    let initialize = |root: &std::path::Path, prefix: &str| {
        let disks = ["a", "b", "c"].map(|slot| directory.path().join(format!("{prefix}-{slot}")));
        Engine::initialize(root, Config::development(disks).unwrap()).unwrap()
    };
    let mut engine = initialize(&source, "source");
    let repository = engine
        .create_repository(
            &storage_engine::RepositoryName::parse("package-fixture").unwrap(),
            "P01",
            3,
            "admin",
            storage_engine::OperationId::generate().unwrap(),
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
            operation_id: storage_engine::OperationId::generate().unwrap(),
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
        Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
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
    let proof_path = package.join("publication.json");
    let mut proof: json_codec::Value =
        json_codec::from_slice(&std::fs::read(&proof_path).unwrap()).unwrap();
    proof["manifest_hash"] = json_codec::Value::String("0".repeat(64));
    std::fs::write(&proof_path, json_codec::to_vec(&proof).unwrap()).unwrap();
    let output = invoke(&source, &["check".as_ref(), package.as_os_str()]);
    assert!(!output.status.success());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(
        diagnostic.contains("manifest is not bound by the published candidate"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("caused by:"), "{diagnostic}");
    assert!(
        diagnostic.contains("immutable object digest differs"),
        "{diagnostic}"
    );
    std::fs::write(package.join("publication.json"), b"invalid proof").unwrap();
    assert!(
        !invoke(&source, &["check".as_ref(), package.as_os_str()])
            .status
            .success()
    );
}

#[test]
fn configuration_commands_edit_only_an_uninitialized_draft() {
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "configuration_commands_edit_only_an_uninitialized_draft",
    ) {
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path();
    let output = directory.join("custom.json");
    let disks = [
        directory.join("p"),
        directory.join("b"),
        directory.join("c"),
    ];
    let binary = env!("CARGO_BIN_EXE_zxs-helper");
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
        .arg(storage_engine::new_id("disk").unwrap())
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
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "template_cli_accepts_one_disk_and_rejects_no_disks",
    ) {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let disk = directory.path().join("only");
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
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
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
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
        let output = Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
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
        let output = Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
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
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "scrub_is_a_real_exclusive_offline_consumer_with_explicit_partial_failure",
    ) {
        return;
    }
    use storage_engine::{Config, Engine, OperationId, RepositoryName};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("system");
    let disks = ["a", "b", "c"].map(|name| directory.path().join(name));
    let mut engine =
        Engine::initialize(&root, Config::development(disks.clone()).unwrap()).unwrap();
    engine
        .create_repository(
            &RepositoryName::parse("scrub-fixture").unwrap(),
            "P01",
            3,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
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
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "scrub_cannot_turn_a_failed_open_into_a_healthy_report",
    ) {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("absent-system");
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
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
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
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
    assert!(help.contains("Usage: zxs-helper scrub [OPTIONS]"));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn offline_export_rejects_non_utf8_receipt_paths_before_io() {
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "offline_export_rejects_non_utf8_receipt_paths_before_io",
    ) {
        return;
    }
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let directory = tempfile::tempdir().unwrap();
    let destination = directory
        .path()
        .join(OsString::from_vec(b"export-\xff".to_vec()));
    let output = Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
        .arg("offline-export")
        .arg(directory.path().join("missing-disk"))
        .arg("ver_00000000000000000000000000000000")
        .arg(&destination)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("output path must be valid UTF-8"), "{error}");
    assert!(!error.contains("panicked"), "{error}");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn offline_export_receipt_paths_are_admitted_before_publishing_verified_bytes() {
    if !service_namespace::enter(
        HOST_NAMESPACE_SCRIPT,
        "offline_export_receipt_paths_are_admitted_before_publishing_verified_bytes",
    ) {
        return;
    }
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    use storage_engine::{CommitRequest, Config, Engine, OperationId, RepositoryName};

    let directory = tempfile::tempdir().unwrap();
    let disk = directory.path().join("disk");
    let mut engine = Engine::initialize(
        &directory.path().join("system"),
        Config::development(vec![disk.clone()]).unwrap(),
    )
    .unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("export-fixture").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "file.txt",
            b"verified export bytes",
        )
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
    let version = engine
        .list_tree(&repository.main_branch_id)
        .unwrap()
        .remove(0)
        .version_id;
    drop(engine);
    let invoke = |destination: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_zxs-helper"))
            .arg("offline-export")
            .arg(&disk)
            .arg(&version)
            .arg(destination)
            .output()
            .unwrap()
    };
    let exports = directory.path().join("exports");
    let invalid = exports.join(OsString::from_vec(b"export-\xff".to_vec()));
    let output = invoke(&invalid);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("output path must be valid UTF-8")
    );
    assert!(
        !exports.exists(),
        "invalid receipt paths must not create output directories"
    );

    let valid = exports.join("导出.txt");
    let output = invoke(&valid);
    assert!(output.status.success(), "{output:?}");
    let receipt: json_codec::Value = json_codec::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["version_id"], version);
    assert_eq!(receipt["exported"], valid.to_str().unwrap());
    assert_eq!(receipt["mode"], "offline-isolated");
    assert_eq!(std::fs::read(&valid).unwrap(), b"verified export bytes");
    let refused = invoke(&valid);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        String::from_utf8(refused.stderr)
            .unwrap()
            .contains("DESTINATION_EXISTS")
    );
    assert_eq!(std::fs::read(&valid).unwrap(), b"verified export bytes");
}
