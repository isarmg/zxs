//! Full ordinary Backup transaction fixtures. Source only under the current
//! no-local-build instruction; these cases have not been compiled or executed.
//! Temporary directories only, never deployed disks or power-loss certification.
use super::*;
use std::io::Read;
use storage_engine::{BackupTopologyTarget, BackupTransactionRequest};

fn full_route() -> (tempfile::TempDir, Engine, Repository) {
    let (temp, mut engine, _) = setup();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("WholeRoute").unwrap(),
            "P01",
            3,
            "admin",
            op(),
        )
        .unwrap();
    (temp, engine, repository)
}

fn transaction(engine: &Engine, paths: Vec<PathBuf>) -> BackupTransactionRequest {
    BackupTransactionRequest {
        preparation: request(engine, paths.clone(), op()),
        targets: paths
            .iter()
            .enumerate()
            .map(|(index, _)| BackupTopologyTarget {
                disk_id: storage_engine::new_id("disk").unwrap(),
                slot_id: format!("C{:02}", index + 2),
                upstream_slot_id: "B01".into(),
            })
            .collect(),
    }
}

fn prepared(engine: &mut Engine, request: &BackupTransactionRequest) {
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
        let status = engine.wait_backup_preparation().unwrap();
        assert_eq!(status.phase, phase, "{status:?}");
        assert!(engine.membership_fence_status().is_none());
        assert!(!engine.state().operations.contains_key(&operation));
        if phase != BackupPreparationPhase::CutoverInputsPrepared {
            engine
                .continue_backup_transaction(operation, "admin")
                .unwrap();
        }
    }
    // A non-decision continuation cannot implicitly stop the source.
    assert_eq!(
        engine
            .continue_backup_transaction(operation, "admin")
            .unwrap()
            .phase,
        BackupPreparationPhase::CutoverInputsPrepared
    );
    assert!(engine.membership_fence_status().is_none());
    engine
        .begin_backup_cutover_fence(operation, "admin")
        .unwrap();
    engine
        .continue_backup_transaction(operation, "admin")
        .unwrap();
    let status = engine.wait_backup_preparation().unwrap();
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
    // Genesis has no generation directory before the first native vote.
    // Inspecting the original slots must admit that missing ancestor without
    // creating it before every original member has passed preflight.
    for disk in &engine.config().disks {
        assert!(!disk.root.join("metadata/membership-generations").exists());
    }
    engine
        .start_backup_registry_placement_decision(operation, "admin")
        .unwrap();
    let status = engine.wait_backup_preparation().unwrap();
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
}

fn body(engine: &Engine, version: &str) -> Vec<u8> {
    let mut content = engine.open_content(version).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn same_backup_switch_serves_and_replays_normal_successor_publications() {
    let (temp, mut engine, repository) = full_route();
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
    let request = transaction(
        &engine,
        vec![
            target(temp.path(), "selected-one"),
            target(temp.path(), "selected-two"),
        ],
    );
    let operation = request.preparation.operation_id;
    prepared(&mut engine, &request);
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
    let result = engine
        .complete_backup_registry_placement_decision(operation, "admin")
        .unwrap();
    assert_eq!(result.kind, "backup-target-replacement");
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
    assert_eq!(
        engine
            .complete_backup_registry_placement_decision(operation, "admin")
            .unwrap(),
        result
    );
    // An exact retried Start does not demand now-nonempty directories or start
    // a new worker. No caller-supplied replacement FD is consumed.
    assert_eq!(
        engine
            .start_backup_transaction(request.clone(), Vec::new())
            .unwrap()
            .phase,
        BackupPreparationPhase::GenerationServingActive
    );
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
    let after = save(
        &mut engine,
        &repository,
        "nested/after",
        b"ordinary successor body",
    );
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
    drop(engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(engine.operation(operation).unwrap(), result);
    assert_eq!(
        engine.backup_preparation_status().unwrap().phase,
        BackupPreparationPhase::GenerationServingActive
    );
    assert_eq!(body(&engine, &old.version_id), b"retained first body");
    assert_eq!(body(&engine, &after.version_id), b"ordinary successor body");
    save(
        &mut engine,
        &repository,
        "nested/again",
        b"writer survives cold replay",
    );
}

#[test]
fn a_changed_original_request_or_principal_never_reselects_targets() {
    let (temp, mut engine, repository) = full_route();
    save(&mut engine, &repository, "file", b"original");
    let request = transaction(&engine, vec![target(temp.path(), "exact-target")]);
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
    prepared(&mut engine, &request);
    engine
        .complete_backup_registry_placement_decision(request.preparation.operation_id, "admin")
        .unwrap();
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
