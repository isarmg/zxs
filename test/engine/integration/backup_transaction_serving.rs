//! Full ordinary Backup transaction fixtures. Source only under the current
//! no-local-build instruction; these cases have not been compiled or executed.
//! Temporary directories only, never deployed disks or power-loss certification.
use super::*;
use std::io::Read;
use storage_engine::{BackupTopologyTarget, BackupTransactionRequest};

// Write directly to the process stream: libtest retains eprintln! output until
// a test returns, which would hide the last completed stage on a CI timeout.
fn diagnostic(start: std::time::Instant, stage: &str) {
    use std::io::Write;
    let _ = writeln!(
        std::io::stderr().lock(),
        "[backup transaction +{:?}] {stage}",
        start.elapsed()
    );
}

struct ProgressDiagnostic {
    stop: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ProgressDiagnostic {
    fn start(system: PathBuf, started: std::time::Instant) -> Self {
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let path = system.join("backup-preparation/progress.json");
            let mut previous = None;
            loop {
                // This is an observer only. Bound diagnostic reads separately
                // from the production progress reader; never create the file.
                if let Ok(file) = fs::File::open(&path) {
                    let mut bytes = Vec::new();
                    if file.take(64 * 1024 + 1).read_to_end(&mut bytes).is_ok()
                        && bytes.len() <= 64 * 1024
                        && previous.as_ref() != Some(&bytes)
                    {
                        diagnostic(
                            started,
                            &format!("durable progress: {}", String::from_utf8_lossy(&bytes)),
                        );
                        previous = Some(bytes);
                    }
                }
                match stopped.recv_timeout(std::time::Duration::from_secs(2)) {
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        Self {
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for ProgressDiagnostic {
    fn drop(&mut self) {
        // Disconnect wakes recv_timeout immediately, including during unwinding.
        // Join before TempDir teardown; no diagnostic worker leaks to other tests.
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn full_route() -> (tempfile::TempDir, Engine, Repository) {
    let (temp, mut engine, repository) = setup();
    let policy = storage_engine::new_id("policy").unwrap();
    engine.update_protection_policy(storage_engine::ProtectionPolicyUpdate {
        policy_id: &policy,
        repository_id: &repository.repository_id,
        path_prefix: "",
        target_copies: 3,
        expected_generation: 0,
        principal: "admin",
        operation_id: op(),
    }).unwrap();
    (temp, engine, repository)
}

fn transaction(engine: &Engine, paths: Vec<PathBuf>) -> BackupTransactionRequest {
    BackupTransactionRequest {
        preparation: request(engine, paths.clone(), op()),
        targets: paths
            .iter()
            .enumerate()
            .map(|(index, _)| BackupTopologyTarget {
                name: name_policy::Name::parse(&format!("New archive {}", index + 1)).unwrap(),
                disk_id: storage_engine::new_id("disk").unwrap(),
                slot_id: format!("C{:02}", index + 2),
                upstream_slot_id: "B01".into(),
            })
            .collect(),
    }
}

fn prepared(engine: &mut Engine, request: &BackupTransactionRequest) {
    let started = std::time::Instant::now();
    diagnostic(started, "prepared: start transaction");
    let operation = request.preparation.operation_id;
    engine
        .start_backup_transaction(
            request.clone(),
            request
                .preparation
                .target_paths
                .iter()
                .map(|path| fs::File::open(path).unwrap())
                .collect(),
        )
        .unwrap();
    for phase in [
        BackupPreparationPhase::HistoryPrepared,
        BackupPreparationPhase::NativeControlPrepared,
        BackupPreparationPhase::ObjectDatabasesPrepared,
        BackupPreparationPhase::ScopedDatabasesPrepared,
        BackupPreparationPhase::CutoverInputsPrepared,
    ] {
        diagnostic(started, &format!("prepared: wait for {phase:?}"));
        let status = engine.wait_backup_preparation().unwrap();
        diagnostic(started, &format!("prepared: observed {:?}", status.phase));
        assert_eq!(status.phase, phase, "{status:?}");
        assert!(engine.membership_fence_status().is_none());
        assert!(!engine.state().operations.contains_key(&operation));
        if phase != BackupPreparationPhase::CutoverInputsPrepared {
            diagnostic(started, "prepared: continue transaction");
            engine
                .continue_backup_transaction(operation, "admin")
                .unwrap();
        }
    }
    // A non-decision continuation cannot implicitly stop the source.
    diagnostic(started, "prepared: verify non-decision continuation");
    assert_eq!(
        engine
            .continue_backup_transaction(operation, "admin")
            .unwrap()
            .phase,
        BackupPreparationPhase::CutoverInputsPrepared
    );
    assert!(engine.membership_fence_status().is_none());
    diagnostic(started, "prepared: begin source cutover fence");
    engine
        .begin_backup_cutover_fence(operation, "admin")
        .unwrap();
    diagnostic(started, "prepared: continue fenced transaction");
    engine
        .continue_backup_transaction(operation, "admin")
        .unwrap();
    diagnostic(started, "prepared: wait for target entities");
    let status = engine.wait_backup_preparation().unwrap();
    diagnostic(started, &format!("prepared: observed {:?}", status.phase));
    assert_eq!(
        status.phase,
        BackupPreparationPhase::TargetEntitiesPrepared,
        "{status:?}"
    );
    assert_eq!(
        engine
            .continue_backup_transaction(operation, "admin")
            .unwrap()
            .phase,
        BackupPreparationPhase::TargetEntitiesPrepared
    );
    // Genesis has no generation or feedback parents before the first native
    // vote. Read-only inspection must admit absent ancestors without creating
    // them before every original member has passed the preparation preflight.
    for disk in &engine.config().disks {
        for relative in [
            "metadata/membership-generations",
            "metadata/backup-root-preparations",
            "metadata/backup-layout-preparations",
        ] {
            assert!(!disk.root.join(relative).exists(), "unexpected prebuilt {relative}");
        }
    }
    diagnostic(started, "prepared: start registry placement decision");
    engine
        .start_backup_registry_placement_decision(operation, "admin")
        .unwrap();
    diagnostic(started, "prepared: wait for ordinary serving installation");
    let status = engine.wait_backup_preparation().unwrap();
    diagnostic(started, &format!("prepared: observed {:?}", status.phase));
    assert_eq!(
        status.phase,
        BackupPreparationPhase::GenerationOrdinaryServingInstalled,
        "{status:?}"
    );
    assert!(engine.is_read_only());
    // A complete native first receipt is deliberately not serving success.
    assert_eq!(
        engine
            .continue_backup_transaction(operation, "admin")
            .unwrap()
            .phase,
        BackupPreparationPhase::GenerationOrdinaryServingInstalled
    );
    diagnostic(started, "prepared: complete");
}

fn body(engine: &Engine, version: &str) -> Vec<u8> {
    let mut content = engine.open_content(version).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    bytes
}

// Assertions derive expected addresses only. Production admission still consumes
// the exact decided five-area and parent occurrences, never these test strings.
fn target_occurrences(engine: &Engine, request: &BackupTransactionRequest, file: &Entry) {
    use std::os::unix::fs::MetadataExt;
    let owner = engine.config().disks.iter().find(|disk| disk.slot_id == "P01").unwrap();
    let direct = engine.config().disks.iter().find(|disk| disk.slot_id == "B01").unwrap();
    let record = &engine.state().file_containers[&file.file_id.parse().unwrap()];
    let nested = engine.state().namespace_directories.values()
        .find(|record| record.name.as_str() == "nested").unwrap();
    let empty = engine.state().namespace_directories.values()
        .find(|record| record.name.as_str() == "empty").unwrap();
    use storage_contract::file_container::ContainerEntity;
    let require_entity = |path: &Path, expected: ContainerEntity| {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(metadata.is_dir());
        assert!(!metadata.file_type().is_symlink());
        assert_eq!((metadata.dev(), metadata.ino()), (expected.device(), expected.inode()));
    };
    for target in &request.targets {
        let disk = engine.config().disks.iter()
            .find(|disk| disk.disk_id == target.disk_id).unwrap();
        let id = disk.disk_id.parse().unwrap();
        let replica = &record.replicas[&id];
        assert_eq!(replica.prefixes.len(), 2);
        assert_eq!(replica.prefixes[0].source_disk.to_string(), direct.disk_id);
        assert_eq!(replica.prefixes[1].source_disk.to_string(), owner.disk_id);
        let mut path = disk.root.clone();
        for (prefix, source) in replica.prefixes.iter().zip([direct, owner]) {
            let uuid = source.disk_id.strip_prefix("disk_").unwrap();
            path.push(format!("{}--{uuid}", source.name));
            require_entity(&path, prefix.entity);
            for (relative, entity) in [
                ("common-data", prefix.areas.common_data),
                ("deleted-data", prefix.areas.deleted_data),
                ("zxs-data", prefix.areas.zxs_data.root),
                ("zxs-data/common-data", prefix.areas.zxs_data.common_data),
                ("zxs-data/deleted-data", prefix.areas.zxs_data.deleted_data),
            ] {
                require_entity(&path.join(relative), entity);
            }
        }
        let common = path.join("common-data");
        require_entity(&common.join("nested"), nested.replicas[&id].entity);
        require_entity(&common.join("nested/empty"), empty.replicas[&id].entity);
        assert_eq!(fs::read_dir(common.join("nested/empty")).unwrap().count(), 0);
        let container = common.join("nested/file");
        require_entity(&container, replica.entity);
        assert!(container.join(".zxs-file.sqlite").is_file());
        let mut bodies = fs::read_dir(container.join("versions")).unwrap()
            .map(|entry| fs::read(entry.unwrap().path()).unwrap()).collect::<Vec<_>>();
        bodies.sort();
        let mut expected = vec![b"retained first body".to_vec(), b"retained current body".to_vec()];
        expected.sort();
        assert_eq!(bodies, expected);
        let metadata = path.join("zxs-data/common-data").join(record.history_tree_id.to_string());
        assert!(metadata.join("versions").is_dir());
        assert!(!disk.root.join("common-data/nested/file").exists());
    }
}

#[test]
fn same_backup_switch_serves_and_replays_normal_successor_publications() {
    let started = std::time::Instant::now();
    diagnostic(started, "main: create full route");
    let (temp, mut engine, repository) = full_route();
    let _progress = ProgressDiagnostic::start(temp.path().join("system"), started);
    diagnostic(started, "main: create original publications");
    engine.file_namespace(
        &repository.repository_id,
        storage_engine::NamespaceOperation::parse(
            storage_engine::NamespaceOperationKind::Mkdir, "nested", None,
        ).unwrap(),
        "admin",
        op(),
    ).unwrap();
    let old = save(
        &mut engine,
        &repository,
        "nested/file",
        b"retained first body",
    );
    let latest = save(
        &mut engine,
        &repository,
        "nested/file",
        b"retained current body",
    );
    engine
        .file_namespace(
            &repository.repository_id,
            storage_engine::NamespaceOperation::parse(
                storage_engine::NamespaceOperationKind::Mkdir,
                "nested/empty",
                None,
            )
            .unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let before = engine.state().operations.clone();
    let originals = engine.config().disks.clone();
    let discovery = fs::read(temp.path().join("system/config.json")).unwrap();
    let request = transaction(
        &engine,
        vec![
            target(temp.path(), "selected-one"),
            target(temp.path(), "selected-two"),
        ],
    );
    let operation = request.preparation.operation_id;
    diagnostic(started, "main: prepare transaction");
    prepared(&mut engine, &request);
    diagnostic(started, "main: inspect published configuration");
    // The admitted source chain already runs primary-first. The published
    // configuration must preserve it before appending the selected fanout.
    let configured = engine
        .inspect_backup_published_configuration(operation, "admin")
        .unwrap();
    assert_eq!(configured.complete_data_route, ["P01", "B01", "C02", "C03"]);
    assert_eq!(configured.retained_replaced_terminal.slot_id, "C01");
    assert_eq!(
        json_codec::to_value(&engine.config().disks).unwrap(),
        json_codec::to_value(&originals).unwrap()
    );
    assert!(engine.is_read_only());
    // The native first Publication has decided the target parent inodes, but
    // no common serving selector has been installed. Equal empty bytes cannot
    // stand in for the recorded ordinary directory occurrence.
    let owner = engine.config().disks.iter().find(|disk| disk.slot_id == "P01").unwrap();
    let direct = engine.config().disks.iter().find(|disk| disk.slot_id == "B01").unwrap();
    let empty = request.preparation.target_paths[0]
        .join(format!("{}--{}", direct.name, direct.disk_id.strip_prefix("disk_").unwrap()))
        .join(format!("{}--{}", owner.name, owner.disk_id.strip_prefix("disk_").unwrap()))
        .join("common-data/nested/empty");
    let parked = temp.path().join("parked-terminal-empty");
    let selectors = originals.iter().map(|disk| {
        let path = disk.root.join("disk.json");
        let bytes = fs::read(&path).unwrap();
        (path, bytes)
    }).collect::<Vec<_>>();
    assert_eq!(fs::read_dir(&empty).unwrap().count(), 0);
    fs::rename(&empty, &parked).unwrap();
    fs::create_dir(&empty).unwrap();
    let error = engine
        .complete_backup_registry_placement_decision(operation, "admin")
        .unwrap_err();
    // The named held-parent check rejects the replacement inode before FILE
    // layout admission. This is a disk identity failure, not a layout fallback.
    assert_eq!(error.code(), ErrorCode::DiskIdentityMismatch, "{error:?}");
    assert!(matches!(&error, storage_engine::Error::FreshLayout(_)), "{error:?}");
    assert!(engine.is_read_only());
    assert!(!engine.state().operations.contains_key(&operation));
    assert_eq!(engine.state().operations, before);
    for (path, bytes) in selectors {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert_eq!(
        fs::read(temp.path().join("system/config.json")).unwrap(),
        discovery
    );
    fs::remove_dir(&empty).unwrap();
    fs::rename(&parked, &empty).unwrap();
    #[cfg(feature = "fault-injection")]
    {
        // Reuse this one real native Publication and serving installation. Cut
        // after the exact bootstrap extension is selected, before its first
        // common selector exchange; completed-cold replay cannot cover this.
        let point = "after_backup_bootstrap_selected";
        engine.inject_fault_once(point);
        let error = engine
            .complete_backup_registry_placement_decision(operation, "admin")
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::InjectedFailure, "{error:?}");
        assert!(error.to_string().contains(&format!("interrupted at {point}")));
        assert!(engine.is_read_only());
        assert!(!engine.state().operations.contains_key(&operation));
        let system = temp.path().join("system");
        let extension = system.join("backup-root-extension");
        assert!(extension.join("selected.json").is_file());
        assert!(!extension.join("complete.json").exists());
        assert!(!system.join("backup-serving-installed.json").exists());
        let selected = fs::read(extension.join("selected.json")).unwrap();
        let plan = fs::read(extension.join("plan.json")).unwrap();
        let seals = originals.iter().map(|disk| {
            let path = disk.root.join("disk.json");
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        }).collect::<Vec<_>>();
        drop(engine);
        assert!(Engine::open(&system).is_err());

        // The unfinished cold branch must verify the same target occurrence,
        // not infer an empty directory or fall back to a legacy layout recipe.
        fs::rename(&empty, &parked).unwrap();
        fs::create_dir(&empty).unwrap();
        let error = Engine::resume_backup_transaction_at_with_workspace_limits(
            &system, operation, "admin", None,
        ).err().expect("a replaced selected target parent cannot resume");
        assert_eq!(error.code(), ErrorCode::ContainerLayoutInvalid, "{error:?}");
        assert!(!extension.join("complete.json").exists());
        assert!(!system.join("backup-serving-installed.json").exists());
        assert_eq!(fs::read(extension.join("selected.json")).unwrap(), selected);
        assert_eq!(fs::read(extension.join("plan.json")).unwrap(), plan);
        for (path, bytes) in seals {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        fs::remove_dir(&empty).unwrap();
        fs::rename(&parked, &empty).unwrap();
        diagnostic(started, "main: resume unfinished exact switch cold");
        engine = Engine::resume_backup_transaction_at_with_workspace_limits(
            &system, operation, "admin", None,
        ).unwrap();
        assert!(extension.join("complete.json").is_file());
        assert_eq!(fs::read(extension.join("selected.json")).unwrap(), selected);
        assert_eq!(fs::read(extension.join("plan.json")).unwrap(), plan);
        assert_eq!(engine.operation(operation).unwrap().operation_id, operation);
    }
    diagnostic(started, "main: complete registry placement decision");
    let result = engine
        .complete_backup_registry_placement_decision(operation, "admin")
        .unwrap();
    diagnostic(started, "main: registry placement decision completed");
    assert_eq!(result.kind, "backup-target-replacement");
    assert_eq!(
        fs::read(temp.path().join("system/config.json")).unwrap(),
        discovery
    );
    assert_eq!(
        engine.backup_preparation_status().unwrap().phase,
        BackupPreparationPhase::GenerationServingActive
    );
    assert!(!engine.is_read_only());
    assert_eq!(
        engine.current_backup_transaction_request("admin").unwrap(),
        Some(request.clone())
    );
    assert_eq!(
        engine
            .backup_transaction_request(operation, "admin")
            .unwrap(),
        Some(request.clone())
    );
    diagnostic(started, "main: retry completed registry placement decision");
    assert_eq!(
        engine
            .complete_backup_registry_placement_decision(operation, "admin")
            .unwrap(),
        result
    );
    // An exact retried Start does not demand now-nonempty directories or start
    // a new worker. No caller-supplied replacement FD is consumed.
    diagnostic(started, "main: retry completed transaction start");
    assert_eq!(
        engine
            .start_backup_transaction(request.clone(), Vec::new())
            .unwrap()
            .phase,
        BackupPreparationPhase::GenerationServingActive
    );
    diagnostic(started, "main: read retained bodies and receipts");
    target_occurrences(&engine, &request, &old);
    assert_eq!(body(&engine, &old.version_id), b"retained first body");
    assert_eq!(body(&engine, &latest.version_id), b"retained current body");
    for (id, receipt) in before.iter() {
        assert_eq!(engine.operation(*id).unwrap(), *receipt);
    }
    for disk in &originals {
        assert!(
            disk.root
                .join("backup-serving-seals")
                .join(operation.to_string())
                .is_dir()
        );
    }
    assert_eq!(
        engine
            .new_file_placement(&repository.repository_id, "nested/after")
            .unwrap(),
        ("P01".into(), 4)
    );
    diagnostic(started, "main: save ordinary successor");
    let after = save(
        &mut engine,
        &repository,
        "nested/after",
        b"ordinary successor body",
    );
    diagnostic(started, "main: ordinary successor saved");
    let placement = &engine.state().placements[&after.version_id];
    assert_eq!(placement.route_slot_ids, ["P01", "B01", "C02", "C03"]);
    assert_eq!(placement.target_copies, 4);
    assert!(!placement.route_slot_ids.iter().any(|slot| slot == "C01"));
    for slot in ["C02", "C03"] {
        assert!(engine.state().file_backup_paths[&after.file_id][slot].ends_with("/nested/after"));
    }
    assert_eq!(
        engine.backup_preparation_status().unwrap().phase,
        BackupPreparationPhase::GenerationServingActive
    );
    let system = temp.path().join("system");
    diagnostic(started, "main: drop live engine");
    drop(engine);
    diagnostic(started, "main: open cold engine");
    let mut engine = Engine::open(&system).unwrap();
    diagnostic(started, "main: cold engine opened");
    target_occurrences(&engine, &request, &old);
    assert_eq!(engine.operation(operation).unwrap(), result);
    for target in &request.targets {
        let disk = engine.config().disks.iter()
            .find(|disk| disk.disk_id == target.disk_id).unwrap();
        assert_eq!(disk.name, target.name);
    }
    assert_eq!(
        engine.backup_preparation_status().unwrap().phase,
        BackupPreparationPhase::GenerationServingActive
    );
    assert_eq!(body(&engine, &old.version_id), b"retained first body");
    assert_eq!(body(&engine, &after.version_id), b"ordinary successor body");
    diagnostic(started, "main: save successor after cold replay");
    save(
        &mut engine,
        &repository,
        "nested/again",
        b"writer survives cold replay",
    );
    diagnostic(started, "main: successor after cold replay saved");
    drop(engine);
    diagnostic(started, "main: cold engine dropped");
}

#[test]
fn a_changed_original_request_or_principal_never_reselects_targets() {
    let (temp, mut engine, repository) = full_route();
    save(&mut engine, &repository, "file", b"original");
    let request = transaction(&engine, vec![target(temp.path(), "exact-target")]);
    let mut duplicate = request.clone();
    duplicate.targets[0].name = engine.config().disks[0].name.clone();
    assert_eq!(
        engine.start_backup_transaction(duplicate, Vec::new()).unwrap_err().code(),
        ErrorCode::NameConflict
    );
    assert!(engine.current_backup_transaction_request("admin").unwrap().is_none());
    engine
        .start_backup_transaction(
            request.clone(),
            vec![fs::File::open(&request.preparation.target_paths[0]).unwrap()],
        )
        .unwrap();
    engine.wait_backup_preparation().unwrap();
    let before = fs::read(
        temp.path()
            .join("system/backup-preparation/transaction.json"),
    )
    .unwrap();
    let mut renamed = request.clone();
    renamed.targets[0].name = name_policy::Name::parse("Changed after retention").unwrap();
    assert_eq!(
        engine.start_backup_transaction(renamed, Vec::new()).unwrap_err().code(),
        ErrorCode::IdempotencyKeyReused
    );
    let mut changed = request.clone();
    changed.targets[0].slot_id = "D99".into();
    assert_eq!(
        engine
            .start_backup_transaction(changed, Vec::new())
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    assert!(engine.current_backup_transaction_request("other").is_err());
    assert!(engine.backup_transaction_request(op(), "admin").is_err());
    assert!(
        engine
            .resume_backup_transaction(request.preparation.operation_id, "other")
            .is_err()
    );
    assert_eq!(
        fs::read(
            temp.path()
                .join("system/backup-preparation/transaction.json")
        )
        .unwrap(),
        before
    );
    assert!(engine.membership_fence_status().is_none());
}

#[test]
fn completed_switch_missing_target_selector_is_not_recreated_by_cold_open() {
    let (temp, mut engine, repository) = full_route();
    save(&mut engine, &repository, "file", b"not a repair permission");
    let request = transaction(&engine, vec![target(temp.path(), "selected")]);
    prepared(&mut engine, &request);
    let operation = request.preparation.operation_id;
    engine
        .complete_backup_registry_placement_decision(operation, "admin")
        .unwrap();
    drop(engine);
    let selector =
        request.preparation.target_paths[0].join("metadata/pool-membership/generation.json");
    let retained = selector.with_file_name("retained-selector.json");
    fs::rename(&selector, &retained).unwrap();
    let bytes = fs::read(&retained).unwrap();
    assert!(Engine::open(&temp.path().join("system")).is_err());
    assert!(
        Engine::resume_backup_transaction_at_with_workspace_limits(
            &temp.path().join("system"),
            operation,
            "admin",
            None
        )
        .is_err()
    );
    assert!(!selector.exists());
    assert_eq!(fs::read(retained).unwrap(), bytes);
}

/// Build the valid durable prefix immediately after bootstrap extension publish
/// and before its last complete marker. Removing only this fixture marker is
/// not a claim to emulate filesystem power loss or to authorize missing data.
fn late_bootstrap_prefix() -> (tempfile::TempDir, BackupTransactionRequest, Entry) {
    let (temp, mut engine, repository) = full_route();
    let old = save(&mut engine, &repository, "kept", b"same original body");
    let request = transaction(&engine, vec![target(temp.path(), "cold-original-target")]);
    let discovery = fs::read(temp.path().join("system/config.json")).unwrap();
    prepared(&mut engine, &request);
    engine
        .complete_backup_registry_placement_decision(request.preparation.operation_id, "admin")
        .unwrap();
    assert_eq!(
        fs::read(temp.path().join("system/config.json")).unwrap(),
        discovery
    );
    drop(engine);
    fs::remove_file(
        temp.path()
            .join("system/backup-root-extension/complete.json"),
    )
    .unwrap();
    (temp, request, old)
}

#[test]
fn selected_bootstrap_prefix_resumes_only_the_exact_original_operation() {
    let (temp, request, old) = late_bootstrap_prefix();
    let root = temp.path().join("system");
    assert!(Engine::open(&root).is_err());
    assert!(
        Engine::resume_backup_transaction_at_with_workspace_limits(&root, op(), "admin", None)
            .is_err()
    );
    assert!(
        Engine::resume_backup_transaction_at_with_workspace_limits(
            &root,
            request.preparation.operation_id,
            "another-principal",
            None
        )
        .is_err()
    );
    assert!(!root.join("backup-root-extension/complete.json").exists());
    let engine = Engine::resume_backup_transaction_at_with_workspace_limits(
        &root,
        request.preparation.operation_id,
        "admin",
        None,
    )
    .unwrap();
    assert_eq!(
        engine.backup_preparation_status().unwrap().phase,
        BackupPreparationPhase::GenerationServingActive
    );
    assert_eq!(body(&engine, &old.version_id), b"same original body");
    assert_eq!(
        engine
            .operation(request.preparation.operation_id)
            .unwrap()
            .kind,
        "backup-target-replacement"
    );
    assert!(root.join("backup-root-extension/complete.json").is_file());
}

#[test]
fn unfinished_native_resume_does_not_recreate_a_missing_target_writer_lock() {
    let (temp, request, _) = late_bootstrap_prefix();
    let lock = request.preparation.target_paths[0].join(".writer.lock");
    fs::remove_file(&lock).unwrap();
    let root = temp.path().join("system");
    assert!(
        Engine::resume_backup_transaction_at_with_workspace_limits(
            &root,
            request.preparation.operation_id,
            "admin",
            None
        )
        .is_err()
    );
    assert!(!lock.exists());
    assert!(!root.join("backup-root-extension/complete.json").exists());
}

#[cfg(feature = "fault-injection")]
struct InterruptedServing {
    temp: tempfile::TempDir,
    engine: Engine,
    repository: Repository,
    request: BackupTransactionRequest,
    old: Entry,
}

#[cfg(feature = "fault-injection")]
fn interrupted_serving(point: &str) -> InterruptedServing {
    let (temp, mut engine, repository) = full_route();
    let old = save(&mut engine, &repository, "kept", b"original retained body");
    let request = transaction(&engine, vec![target(temp.path(), "retained-target")]);
    prepared(&mut engine, &request);
    engine.inject_fault_once(point);
    assert_eq!(
        engine
            .complete_backup_registry_placement_decision(request.preparation.operation_id, "admin",)
            .unwrap_err()
            .code(),
        ErrorCode::InjectedFailure,
        "{point}"
    );
    assert!(engine.is_read_only());
    assert!(engine.membership_fence_status().is_some());
    assert_ne!(
        engine.backup_preparation_status().unwrap().phase,
        BackupPreparationPhase::GenerationServingActive
    );
    assert!(Engine::open(&temp.path().join("system")).is_err());
    InterruptedServing {
        temp,
        engine,
        repository,
        request,
        old,
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn live_final_handoff_retries_keep_the_original_operation_and_writers() {
    for point in [
        "after_backup_serving_selected",
        "after_backup_bootstrap_published",
        "after_backup_serving_resources",
        "after_backup_current_installed",
    ] {
        let InterruptedServing {
            temp,
            mut engine,
            repository,
            request,
            old,
        } = interrupted_serving(point);
        let operation = request.preparation.operation_id;
        assert!(
            engine
                .resume_backup_transaction(operation, "other")
                .is_err()
        );
        assert!(engine.resume_backup_transaction(op(), "admin").is_err());
        assert!(engine.is_read_only());
        assert_eq!(
            engine
                .resume_backup_transaction(operation, "admin")
                .unwrap()
                .phase,
            BackupPreparationPhase::GenerationServingActive,
            "{point}"
        );
        let result = engine.operation(operation).unwrap();
        let sequence = engine.state().sequence;
        let receipts = engine.state().operations.clone();
        assert_eq!(
            engine
                .complete_backup_registry_placement_decision(operation, "admin")
                .unwrap(),
            result
        );
        assert_eq!(engine.state().sequence, sequence);
        assert_eq!(engine.state().operations, receipts);
        assert!(!engine.is_read_only());
        assert_eq!(body(&engine, &old.version_id), b"original retained body");
        let after = save(
            &mut engine,
            &repository,
            "after",
            b"ordinary write after retry",
        );
        drop(engine);
        let engine = Engine::open(&temp.path().join("system")).unwrap();
        assert_eq!(engine.operation(operation).unwrap(), result);
        assert_eq!(body(&engine, &old.version_id), b"original retained body");
        assert_eq!(
            body(&engine, &after.version_id),
            b"ordinary write after retry"
        );
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn completed_bootstrap_failure_resumes_cold_without_another_publication() {
    for point in [
        "after_backup_bootstrap_published",
        "after_backup_serving_resources",
        "after_backup_current_installed",
    ] {
        let InterruptedServing {
            temp,
            engine,
            repository,
            request,
            old,
        } = interrupted_serving(point);
        let operation = request.preparation.operation_id;
        let root = temp.path().join("system");
        assert!(root.join("backup-root-extension/complete.json").is_file());
        drop(engine);
        let mut engine = Engine::resume_backup_transaction_at_with_workspace_limits(
            &root, operation, "admin", None,
        )
        .unwrap();
        let result = engine.operation(operation).unwrap();
        let sequence = engine.state().sequence;
        assert_eq!(
            engine
                .complete_backup_registry_placement_decision(operation, "admin")
                .unwrap(),
            result
        );
        assert_eq!(engine.state().sequence, sequence);
        assert!(!engine.is_read_only());
        assert_eq!(body(&engine, &old.version_id), b"original retained body");
        save(
            &mut engine,
            &repository,
            "after-cold",
            b"cold retry serves writes",
        );
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn live_completed_bootstrap_never_recreates_a_missing_target_selector() {
    let InterruptedServing {
        temp,
        mut engine,
        request,
        ..
    } = interrupted_serving("after_backup_bootstrap_published");
    let operation = request.preparation.operation_id;
    let selector =
        request.preparation.target_paths[0].join("metadata/pool-membership/generation.json");
    let retained = selector.with_file_name("retained-selector.json");
    fs::rename(&selector, &retained).unwrap();
    let bytes = fs::read(&retained).unwrap();
    assert!(
        engine
            .resume_backup_transaction(operation, "admin")
            .is_err()
    );
    assert!(engine.is_read_only());
    assert!(engine.membership_fence_status().is_some());
    assert!(!selector.exists());
    assert_eq!(fs::read(&retained).unwrap(), bytes);
    drop(engine);
    assert!(
        Engine::resume_backup_transaction_at_with_workspace_limits(
            &temp.path().join("system"),
            operation,
            "admin",
            None,
        )
        .is_err()
    );
    assert!(!selector.exists());
    assert_eq!(fs::read(retained).unwrap(), bytes);
}

#[test]
fn successful_backup_inspection_does_not_cache_any_target_completion() {
    let (temp, mut engine, repository) = full_route();
    save(&mut engine, &repository, "kept", b"complete original body");
    let request = transaction(
        &engine,
        vec![
            target(temp.path(), "completion-one"),
            target(temp.path(), "completion-two"),
        ],
    );
    let operation = request.preparation.operation_id;
    prepared(&mut engine, &request);
    engine
        .inspect_backup_published_registry(operation, "admin")
        .unwrap();
    for name in [
        "generation-inputs-prepared.json",
        "generation-control-prepared.json",
        "generation-authority-prepared.json",
        "generation-first-candidate-prepared.json",
        "generation-first-publication-retained.json",
        "published-registry-installed.json",
    ] {
        let first = request.preparation.target_paths[0].join(name);
        let second = request.preparation.target_paths[1].join(name);
        let original = fs::read(&second).unwrap();
        assert_eq!(fs::read(&first).unwrap(), original);
        let mut corrupt = original.clone();
        corrupt[0] ^= 1;
        fs::write(&second, &corrupt).unwrap();
        assert_eq!(
            engine
                .inspect_backup_published_registry(operation, "admin")
                .unwrap_err()
                .code(),
            ErrorCode::ConfigConflict,
            "{name}"
        );
        assert_eq!(fs::read(&second).unwrap(), corrupt);
        assert_eq!(fs::read(&first).unwrap(), original);
        fs::write(&second, &original).unwrap();
        engine
            .inspect_backup_published_registry(operation, "admin")
            .unwrap();
        fs::remove_file(&second).unwrap();
        assert!(
            engine
                .inspect_backup_published_registry(operation, "admin")
                .is_err(),
            "{name}"
        );
        assert!(!second.exists());
        assert_eq!(fs::read(&first).unwrap(), original);
        assert!(engine.is_read_only());
        fs::write(&second, &original).unwrap();
        engine
            .inspect_backup_published_registry(operation, "admin")
            .unwrap();
    }
    assert!(engine.is_read_only());
    assert!(!engine.state().operations.contains_key(&operation));
}
