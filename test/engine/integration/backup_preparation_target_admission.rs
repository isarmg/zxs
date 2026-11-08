//! Real Engine target-admission regressions. HistoryPrepared is not cutover.
//! Temporary media and ordinary cold reopen do not certify physical power loss.
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};
use storage_engine::{
    BackupPreparationPhase, BackupPreparationRequest, CommitRequest, Config, Engine, ErrorCode,
    MembershipExitAction, OperationId, PendingMemberDirectory, Repository, RepositoryName,
    ValidatedConfig, offline_check_backup_preparation, offline_check_history_package,
};

fn operation() -> OperationId {
    OperationId::generate().unwrap()
}
fn authority(engine: &Engine) -> (Vec<u8>, Vec<u8>) {
    (
        storage_engine::canonical_json(engine.state()).unwrap(),
        storage_engine::canonical_json(engine.config()).unwrap(),
    )
}
fn save(engine: &mut Engine, repository: &Repository, bytes: &[u8]) {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file", bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "target admission fixture".into(),
            request_digest: None,
        })
        .unwrap();
}
struct Prepared {
    temp: tempfile::TempDir,
    engine: Engine,
    repository: Repository,
    destination: PathBuf,
    request: BackupPreparationRequest,
    source: (Vec<u8>, Vec<u8>),
    job: Vec<u8>,
    manifest_hash: String,
}
impl Prepared {
    fn new(current: bool) -> Self {
        let temp = tempfile::Builder::new()
            .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .tempdir()
            .unwrap();
        let count = if current { 4 } else { 3 };
        let mut config =
            Config::development((0..count).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        if current {
            config.disks[3].level = 1;
            config.disks[3].upstream_slot_id = None;
            config.disks[3].metadata_member = false;
        }
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("TargetAdmission").unwrap(),
                "P01",
                3,
                "admin",
                operation(),
            )
            .unwrap();
        if current {
            engine
                .begin_disk_exit(
                    &config.disks[3].disk_id,
                    MembershipExitAction::Deregister,
                    "admin",
                    operation(),
                )
                .unwrap();
            let archive = engine.build_managed_membership_control_archive().unwrap();
            engine
                .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
                .unwrap();
            engine.resume_membership_joint_decision().unwrap();
            engine.activate_pending_disk_exit().unwrap();
            assert!(engine.current_disk_registry_commitment().is_some());
        }
        save(&mut engine, &repository, b"retained original full body");
        save(&mut engine, &repository, b"latest independent full body");
        let destination = temp.path().join("prepared-target");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&destination)
            .unwrap();
        let request = BackupPreparationRequest {
            operation_id: operation(),
            principal: "admin".into(),
            source_slot_id: "B01".into(),
            target_paths: vec![destination.clone()],
            expected_sequence: engine.state().sequence,
        };
        let source = authority(&engine);
        engine
            .start_backup_preparation(request.clone(), fs::File::open(&destination).unwrap())
            .unwrap();
        let status = engine.wait_backup_preparation().unwrap();
        assert_eq!(status.phase, BackupPreparationPhase::HistoryPrepared);
        assert_eq!(status.operation_id, request.operation_id);
        assert_eq!(authority(&engine), source);
        let report = offline_check_history_package(
            &destination
                .join("histories")
                .join(&repository.history_tree_id),
        )
        .unwrap();
        assert_eq!(report.full_files, 2);
        offline_check_backup_preparation(&destination).unwrap();
        let job = fs::read(temp.path().join("system/backup-preparation/job.json")).unwrap();
        Self {
            temp,
            engine,
            repository,
            destination,
            request,
            source,
            job,
            manifest_hash: report.manifest_hash,
        }
    }
    fn package(&self) -> PathBuf {
        self.destination
            .join("histories")
            .join(&self.repository.history_tree_id)
    }
}

#[test]
fn prepared_history_verifies_without_original_system_or_storage_roots() {
    let prepared = Prepared::new(false);
    let package = prepared.package();
    let roots = prepared
        .engine
        .config()
        .disks
        .iter()
        .map(|disk| disk.root.clone())
        .chain(std::iter::once(prepared.temp.path().join("system")))
        .collect::<Vec<_>>();
    drop(prepared.engine);
    for (index, root) in roots.iter().enumerate() {
        fs::rename(
            root,
            prepared.temp.path().join(format!("offline-source-{index}")),
        )
        .unwrap();
        assert!(!root.exists());
    }
    offline_check_backup_preparation(&prepared.destination).unwrap();
    let report = offline_check_history_package(&package).unwrap();
    assert_eq!(report.manifest_hash, prepared.manifest_hash);
    assert_eq!(report.full_files, 2);
    // This proves only material independence; the target is not an active member.
    assert!(!prepared.destination.join("disk.json").exists());
}

#[test]
fn durable_prepared_marker_and_full_progress_do_not_hide_corrupt_target_checksums() {
    let prepared = Prepared::new(false);
    let version = prepared.engine.state().versions.values().next().unwrap();
    let checksums = prepared
        .package()
        .join("versions")
        .join(&version.version_id)
        .join("checksums.bin");
    let original = fs::read(&checksums).unwrap();
    assert!(!original.is_empty());
    let mut corrupt = original.clone();
    corrupt[0] ^= 1;
    let marker = fs::read(prepared.destination.join("history-prepared.json")).unwrap();
    let system = prepared.temp.path().join("system");
    let expected_job = prepared.job.clone();
    drop(prepared.engine);
    fs::write(&checksums, &corrupt).unwrap();
    assert_eq!(
        offline_check_backup_preparation(&prepared.destination)
            .unwrap_err()
            .code(),
        ErrorCode::ChecksumMismatch
    );
    let mut engine = Engine::open(&system).unwrap();
    let blocked = engine.wait_backup_preparation().unwrap();
    assert_eq!(blocked.phase, BackupPreparationPhase::Blocked);
    assert_eq!(blocked.error_code, Some(ErrorCode::ChecksumMismatch));
    assert_eq!(blocked.operation_id, prepared.request.operation_id);
    assert_eq!(fs::read(&checksums).unwrap(), corrupt);
    assert_eq!(
        fs::read(system.join("backup-preparation/job.json")).unwrap(),
        expected_job
    );
    assert_eq!(
        fs::read(prepared.destination.join("history-prepared.json")).unwrap(),
        marker
    );
    assert_eq!(authority(&engine), prepared.source);
    // Restore only this fixture's exact original bytes, then reverify the same
    // operation. A completed progress file alone must not admit the target.
    fs::write(&checksums, original).unwrap();
    engine
        .start_backup_preparation(
            prepared.request.clone(),
            fs::File::open(&prepared.destination).unwrap(),
        )
        .unwrap();
    let resumed = engine.wait_backup_preparation().unwrap();
    assert_eq!(resumed.phase, BackupPreparationPhase::HistoryPrepared);
    assert_eq!(resumed.operation_id, prepared.request.operation_id);
    assert_eq!(authority(&engine), prepared.source);
    let report = offline_check_history_package(
        &prepared
            .destination
            .join("histories")
            .join(&prepared.repository.history_tree_id),
    )
    .unwrap();
    assert_eq!(report.manifest_hash, prepared.manifest_hash);
}

#[test]
fn valid_json_marker_for_another_job_blocks_without_overwriting_conflicting_evidence() {
    let prepared = Prepared::new(false);
    let package = prepared.package();
    let marker_path = prepared.destination.join("history-prepared.json");
    let original = fs::read(&marker_path).unwrap();
    let mut marker: json_codec::Value = json_codec::from_slice(&original).unwrap();
    let wrong = "0".repeat(64);
    assert_ne!(marker["job_hash"].as_str().unwrap(), wrong);
    marker["job_hash"] = json_codec::Value::String(wrong);
    let corrupt = storage_engine::canonical_json(&marker).unwrap();
    assert_eq!(corrupt.len(), original.len());
    let system = prepared.temp.path().join("system");
    drop(prepared.engine);
    fs::write(&marker_path, &corrupt).unwrap();
    assert_eq!(
        offline_check_backup_preparation(&prepared.destination)
            .unwrap_err()
            .code(),
        ErrorCode::HistoryPackageInvalid
    );
    // The history package remains valid, but the job-bound completion record is
    // conflicting. The two verification results must not be conflated.
    assert_eq!(
        offline_check_history_package(&package)
            .unwrap()
            .manifest_hash,
        prepared.manifest_hash
    );
    let mut engine = Engine::open(&system).unwrap();
    let blocked = engine.wait_backup_preparation().unwrap();
    assert_eq!(blocked.phase, BackupPreparationPhase::Blocked);
    assert_eq!(blocked.error_code, Some(ErrorCode::ObjectCollision));
    assert_eq!(fs::read(&marker_path).unwrap(), corrupt);
    assert_eq!(
        fs::read(system.join("backup-preparation/job.json")).unwrap(),
        prepared.job
    );
    engine
        .start_backup_preparation(
            prepared.request.clone(),
            fs::File::open(&prepared.destination).unwrap(),
        )
        .unwrap();
    assert_eq!(
        engine.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::Blocked
    );
    assert_eq!(fs::read(&marker_path).unwrap(), corrupt);
    assert_eq!(authority(&engine), prepared.source);
    fs::write(&marker_path, original).unwrap();
    engine
        .start_backup_preparation(
            prepared.request.clone(),
            fs::File::open(&prepared.destination).unwrap(),
        )
        .unwrap();
    let resumed = engine.wait_backup_preparation().unwrap();
    assert_eq!(resumed.phase, BackupPreparationPhase::HistoryPrepared);
    assert_eq!(resumed.operation_id, prepared.request.operation_id);
    assert_eq!(authority(&engine), prepared.source);
}

fn inventory(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn scan(root: &Path, path: &Path, rows: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                rows.insert(path.strip_prefix(root).unwrap().to_path_buf(), None);
                scan(root, &path, rows);
            } else {
                assert!(kind.is_file());
                rows.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    Some(fs::read(&path).unwrap()),
                );
            }
        }
    }
    let mut rows = BTreeMap::new();
    scan(root, root, &mut rows);
    rows
}

#[test]
fn history_prepared_is_not_permission_to_replace_a_registered_disk() {
    for current in [false, true] {
        let mut prepared = Prepared::new(current);
        let before_target = inventory(&prepared.destination);
        let old_state = authority(&prepared.engine);
        for with_directory in [false, true] {
            let mut replacement = prepared.engine.config().clone();
            let disk = replacement
                .disks
                .iter_mut()
                .find(|disk| disk.slot_id == "C01")
                .unwrap();
            disk.disk_id = storage_engine::new_id("disk").unwrap();
            disk.binding_generation += 1;
            disk.root = prepared.destination.clone();
            let new_id = disk.disk_id.clone();
            // Refusal must exercise a structurally valid replacement proposal.
            ValidatedConfig::try_from(replacement.clone()).unwrap();
            let id = operation();
            let result = if with_directory {
                prepared.engine.begin_membership_handoff_with_directories(
                    replacement,
                    "admin",
                    id,
                    vec![PendingMemberDirectory {
                        disk_id: new_id,
                        directory: fs::File::open(&prepared.destination).unwrap(),
                    }],
                )
            } else {
                prepared
                    .engine
                    .begin_membership_handoff(replacement, "admin", id)
            };
            assert_eq!(result.unwrap_err().code(), ErrorCode::ConfigConflict);
            assert!(prepared.engine.membership_fence_status().is_none());
            assert!(!prepared.engine.is_read_only());
            assert!(!prepared.engine.state().operations.contains_key(&id));
            assert_eq!(authority(&prepared.engine), old_state);
            assert_eq!(inventory(&prepared.destination), before_target);
        }
        let system = prepared.temp.path().join("system");
        drop(prepared.engine);
        let mut engine = Engine::open(&system).unwrap();
        let status = engine.wait_backup_preparation().unwrap();
        assert_eq!(status.phase, BackupPreparationPhase::HistoryPrepared);
        assert_eq!(status.operation_id, prepared.request.operation_id);
        assert_eq!(authority(&engine), prepared.source);
        assert_eq!(inventory(&prepared.destination), before_target);
        assert!(!prepared.destination.join("disk.json").exists());
    }
}
