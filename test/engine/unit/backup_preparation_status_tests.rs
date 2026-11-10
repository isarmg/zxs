//! Real frozen task/roots with a controlled worker panic. This tests ownership
//! and failure observation, not a production I/O path causing that panic.
use super::*;
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    time::{Duration, Instant},
};

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    target: PathBuf,
    request: BackupPreparationRequest,
    job: Vec<u8>,
    state: Vec<u8>,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .tempdir()
            .unwrap();
        let config =
            Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
        let repository = engine
            .create_disk_root("P01", "admin", OperationId::generate().unwrap())
            .unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "body", b"frozen panic fixture")
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "worker panic fixture".into(),
                request_digest: None,
            })
            .unwrap();
        let target = temp.path().join("target");
        fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
        let request = BackupPreparationRequest {
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            source_slot_id: "B01".into(),
            target_paths: vec![target.clone()],
            expected_sequence: engine.state.sequence,
        };
        engine
            .start_backup_preparation(request.clone(), File::open(&target).unwrap())
            .unwrap();
        assert_eq!(
            engine.wait_backup_preparation().unwrap().phase,
            BackupPreparationPhase::HistoryPrepared
        );
        let job = fs::read(temp.path().join("system").join(JOB)).unwrap();
        let state = canonical_json(engine.state()).unwrap();
        Self {
            temp,
            engine,
            target,
            request,
            job,
            state,
        }
    }
    fn install_panicking_owner(&mut self, poison: bool) {
        let root = DiskRoot::open(&self.target).unwrap();
        let lock = AdvisoryLock::directory(&root.directory(root.path()).unwrap()).unwrap();
        let locks = self.engine.retain_writer_locks().unwrap();
        let plans = self.engine.backup_history_plans().unwrap();
        let worker = self.engine.backup_preparation.as_mut().unwrap();
        assert!(worker.thread.is_none());
        *worker.state.lock().unwrap() = worker.job.status();
        let state = Arc::clone(&worker.state);
        worker.thread = Some(std::thread::spawn(move || {
            let (_root, _lock, _locks, _plans) = (root, lock, locks, plans);
            if poison {
                let _status = state.lock().unwrap();
                panic!("controlled preparation owner panic with poisoned diagnostic cache");
            }
            panic!("controlled preparation owner panic outside diagnostic cache");
        }));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !worker.thread.as_ref().unwrap().is_finished() {
            assert!(
                Instant::now() < deadline,
                "controlled panic did not terminate"
            );
            std::thread::yield_now();
        }
    }
    fn assert_blocked_and_retained(&self) {
        let status = self.engine.backup_preparation_status().unwrap();
        assert_eq!(status.operation_id, self.request.operation_id);
        assert_eq!(status.phase, BackupPreparationPhase::Blocked);
        assert_eq!(status.error_code, Some(ErrorCode::RecoveryRequired));
        assert_eq!(canonical_json(self.engine.state()).unwrap(), self.state);
        assert_eq!(
            fs::read(self.temp.path().join("system").join(JOB)).unwrap(),
            self.job
        );
    }
    fn assert_target_lock_released(&self) {
        let root = DiskRoot::open(&self.target).unwrap();
        let held = AdvisoryLock::directory(&root.directory(root.path()).unwrap()).unwrap();
        // Positive control: these are distinct open-file descriptions, so a
        // second owner cannot also acquire the still-held lock.
        assert!(AdvisoryLock::directory(&root.directory(root.path()).unwrap()).is_err());
        drop(held);
    }
    fn retry(&mut self) -> Result<BackupPreparationStatus> {
        self.engine
            .start_backup_preparation(self.request.clone(), File::open(&self.target).unwrap())
    }
    fn assert_same_task_completes(mut self) {
        self.retry().unwrap();
        let completed = self.engine.wait_backup_preparation().unwrap();
        assert_eq!(completed.operation_id, self.request.operation_id);
        assert_eq!(completed.phase, BackupPreparationPhase::HistoryPrepared);
        assert_eq!(
            fs::read(self.temp.path().join("system").join(JOB)).unwrap(),
            self.job
        );
        assert_eq!(canonical_json(self.engine.state()).unwrap(), self.state);
        drop(self.engine);
        let mut engine = Engine::open(&self.temp.path().join("system")).unwrap();
        let cold = engine.wait_backup_preparation().unwrap();
        assert_eq!(cold.operation_id, self.request.operation_id);
        assert_eq!(cold.phase, BackupPreparationPhase::HistoryPrepared);
    }
}

#[test]
fn panicking_worker_wait_retains_failure_and_releases_owned_resources() {
    let mut fixture = Fixture::new();
    fixture.install_panicking_owner(false);
    assert_eq!(
        fixture.engine.wait_backup_preparation().unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    fixture.assert_blocked_and_retained();
    assert_eq!(
        fixture.engine.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::Blocked
    );
    fixture.assert_target_lock_released();
    fixture.assert_same_task_completes();
}

#[test]
fn panicking_worker_retry_keeps_the_task_until_exact_reverification() {
    let mut fixture = Fixture::new();
    fixture.install_panicking_owner(false);
    assert_eq!(
        fixture.retry().unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    fixture.assert_blocked_and_retained();
    fixture.assert_target_lock_released();
    fixture.assert_same_task_completes();
}

#[test]
fn poisoned_progress_is_reported_as_unavailable_not_a_healthy_or_missing_task() {
    let mut fixture = Fixture::new();
    fixture.install_panicking_owner(true);
    fixture.assert_blocked_and_retained();
    assert_eq!(
        fixture.engine.wait_backup_preparation().unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    fixture.assert_blocked_and_retained();
    fixture.assert_target_lock_released();
    fixture.assert_same_task_completes();
}

#[test]
fn shared_worker_progress_persists_without_holding_its_cache_lock() {
    let mut fixture = Fixture::new();
    let worker = fixture.engine.backup_preparation.take().unwrap();
    let cache = Arc::clone(&worker.state);
    let mut worker = worker
        .spawn(
            fixture.engine.retain_writer_locks().unwrap(),
            DiskRoot::open(&fixture.engine.root).unwrap(),
            "test-backup-progress",
            move |progress| {
                progress
                    .update(|status| {
                        assert!(cache.try_lock().is_ok());
                        status.detail = "shared progress persisted".into();
                    })
                    .unwrap();
                progress.finish(Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "controlled stage failure",
                )));
            },
        )
        .unwrap();
    worker.join().unwrap();
    let status = worker.status();
    assert_eq!(status.phase, BackupPreparationPhase::Blocked);
    assert_eq!(status.error_code, Some(ErrorCode::ConfigConflict));
    assert!(status.detail.contains("controlled stage failure"));
    let persisted: BackupPreparationStatus =
        crate::journal::decode_proof(&fs::read(fixture.engine.root.join(PROGRESS)).unwrap())
            .unwrap();
    assert_eq!(persisted, status);
    assert_eq!(
        canonical_json(fixture.engine.state()).unwrap(),
        fixture.state
    );
    assert_eq!(
        fs::read(fixture.engine.root.join(JOB)).unwrap(),
        fixture.job
    );
}

#[test]
fn shared_worker_progress_failure_keeps_the_primary_error_and_observed_state() {
    let mut fixture = Fixture::new();
    let progress_path = fixture.engine.root.join(PROGRESS);
    let retained = progress_path.with_extension("retained");
    fs::rename(&progress_path, &retained).unwrap();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&progress_path)
        .unwrap();
    let original = fs::read(&retained).unwrap();
    let mut worker = fixture
        .engine
        .backup_preparation
        .take()
        .unwrap()
        .spawn(
            fixture.engine.retain_writer_locks().unwrap(),
            DiskRoot::open(&fixture.engine.root).unwrap(),
            "test-backup-progress-failure",
            |progress| {
                assert!(
                    progress
                        .update(|status| status.completed_bytes = 7)
                        .is_err()
                );
                progress.finish(Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "controlled stage failure",
                )));
            },
        )
        .unwrap();
    worker.join().unwrap();
    let status = worker.status();
    assert_eq!(status.phase, BackupPreparationPhase::Blocked);
    assert_eq!(status.error_code, Some(ErrorCode::ConfigConflict));
    assert_eq!(status.completed_bytes, 7);
    assert!(status.detail.contains("controlled stage failure"));
    assert!(status.detail.contains("progress persistence failed:"));
    assert_eq!(fs::read(retained).unwrap(), original);
    assert!(progress_path.is_dir());
    assert_eq!(
        canonical_json(fixture.engine.state()).unwrap(),
        fixture.state
    );
    assert_eq!(
        fs::read(fixture.engine.root.join(JOB)).unwrap(),
        fixture.job
    );
}

#[test]
fn shared_worker_drop_cancels_before_joining_and_releasing_target_ownership() {
    let mut fixture = Fixture::new();
    let root = DiskRoot::open(&fixture.target).unwrap();
    let held = AdvisoryLock::directory(&root.directory(root.path()).unwrap()).unwrap();
    let (entered, started) = std::sync::mpsc::sync_channel(0);
    let worker = fixture
        .engine
        .backup_preparation
        .take()
        .unwrap()
        .spawn(
            fixture.engine.retain_writer_locks().unwrap(),
            DiskRoot::open(&fixture.engine.root).unwrap(),
            "test-backup-cancel",
            move |progress| {
                let (_root, _held) = (root, held);
                entered.send(()).unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                while !progress.stop.load(Ordering::Acquire) {
                    assert!(
                        Instant::now() < deadline,
                        "worker did not observe cancellation"
                    );
                    std::thread::yield_now();
                }
                progress.finish(Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "controlled cancellation observed",
                )));
            },
        )
        .unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let observed = Arc::clone(&worker.state);
    let root = DiskRoot::open(&fixture.target).unwrap();
    assert!(AdvisoryLock::directory(&root.directory(root.path()).unwrap()).is_err());
    drop(worker);
    assert!(
        observed
            .lock()
            .unwrap()
            .detail
            .contains("controlled cancellation observed")
    );
    fixture.assert_target_lock_released();
    assert_eq!(
        canonical_json(fixture.engine.state()).unwrap(),
        fixture.state
    );
    assert_eq!(
        fs::read(fixture.engine.root.join(JOB)).unwrap(),
        fixture.job
    );
}

#[test]
fn backup_inspection_entrypoints_preserve_actor_and_preparation_order() {
    let temp = tempfile::tempdir().unwrap();
    let config =
        Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let operation = OperationId::generate().unwrap();
    type Inspect = fn(&mut Engine, OperationId, &str) -> Result<()>;
    let inspections: [Inspect; 11] = [
        |engine, operation, actor| {
            engine
                .inspect_backup_registry_decision(operation, actor)
                .map(drop)
        },
        |engine, operation, actor| {
            engine
                .inspect_backup_first_publication(operation, actor)
                .map(drop)
        },
        |engine, operation, actor| {
            engine
                .inspect_backup_target_publication(operation, actor)
                .map(drop)
        },
        |engine, operation, actor| {
            engine
                .inspect_backup_published_configuration(operation, actor)
                .map(drop)
        },
        |engine, operation, actor| {
            engine
                .inspect_backup_published_registry(operation, actor)
                .map(drop)
        },
        |engine, operation, actor| {
            engine
                .inspect_backup_published_serving_images(operation, actor)
                .map(drop)
        },
        |engine, operation, actor| {
            engine
                .inspect_backup_published_scoped_databases(operation, actor)
                .map(drop)
        },
        |engine, operation, actor| {
            engine
                .inspect_backup_common_databases(operation, actor)
                .map(drop)
        },
        |engine, operation, actor| {
            engine
                .inspect_backup_current_read(operation, actor)
                .map(drop)
        },
        |engine, operation, actor| {
            engine
                .inspect_backup_database_aliases(operation, actor)
                .map(drop)
        },
        |engine, operation, actor| {
            engine
                .inspect_backup_first_finalize(operation, actor)
                .map(drop)
        },
    ];
    engine
        .commit_preparation
        .store(true, std::sync::atomic::Ordering::Release);
    for inspect in &inspections {
        assert_eq!(
            inspect(&mut engine, operation, "").unwrap_err().code(),
            ErrorCode::Unauthenticated
        );
        assert_eq!(
            inspect(&mut engine, operation, "admin").unwrap_err().code(),
            ErrorCode::CommitPreparationBusy
        );
    }
    engine
        .commit_preparation
        .store(false, std::sync::atomic::Ordering::Release);
    for inspect in inspections {
        assert_eq!(
            inspect(&mut engine, operation, "admin").unwrap_err().code(),
            ErrorCode::IoError
        );
    }
    assert!(
        !engine
            .root
            .join("backup-preparation/registry-placement.json")
            .exists()
    );
}
