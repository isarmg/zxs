//! Explicit whole-pool fanout preparation. Every selected target receives the
//! complete unit; this is never a shard allocator, admission or cutover writer.
use super::*;
use std::{collections::BTreeSet, path::Component};

const FORMAT: &str = "zxs-backup-whole-unit-batch";
pub(super) const BATCH_JOB: &str = "backup-preparation/batch.json";
const BATCH_INTENT: &str = "backup-batch.json";
pub(super) const BATCH_COMPLETE: &str = "backup-batch-prepared.json";
const MAX_TARGETS: usize = 8;
const MAX_BATCH_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Batch {
    format: String,
    request: BackupPreparationRequest,
    targets: Vec<Job>,
}

impl Batch {
    pub(super) fn jobs(&self) -> &[Job] {
        &self.targets
    }
    pub(super) fn request(&self) -> &BackupPreparationRequest {
        &self.request
    }

    fn first(&self) -> &Job {
        self.targets
            .first()
            .expect("invariant: admitted nonempty batch")
    }

    pub(super) fn validate(&self) -> Result<()> {
        validate_request(&self.request)?;
        if self.format != FORMAT || self.targets.len() != self.request.target_paths.len() {
            return Err(invalid("backup batch omits or adds a selected target"));
        }
        let first = self.first();
        let mut identities = BTreeSet::new();
        for (job, path) in self.targets.iter().zip(&self.request.target_paths) {
            let expected = single_request(&self.request, path.clone());
            if job.format != super::FORMAT
                || job.request != expected
                || job.pool_id != first.pool_id
                || job.snapshot != first.snapshot
                || job.configuration_hash != first.configuration_hash
                || job.history_roots != first.history_roots
                || job.total_files != first.total_files
                || job.total_bytes != first.total_bytes
                || !identities.insert((job.target.device, job.target.inode))
            {
                return Err(invalid(
                    "backup batch targets do not bind the same complete frozen unit",
                ));
            }
        }
        first
            .total_files
            .checked_mul(self.targets.len() as u64)
            .ok_or_else(|| invalid("backup batch file count overflows"))?;
        first
            .total_bytes
            .checked_mul(self.targets.len() as u64)
            .ok_or_else(|| invalid("backup batch byte count overflows"))?;
        Ok(())
    }

    pub(super) fn status(&self) -> BackupPreparationStatus {
        let mut status = self.first().status();
        // Checked by validate before constructing a worker or observing a job.
        status.total_files *= self.targets.len() as u64;
        status.total_bytes *= self.targets.len() as u64;
        status.detail = format!(
            "preparing {} explicit complete copies; no placement or registration change",
            self.targets.len()
        );
        status
    }

    fn encoded(&self) -> Result<Vec<u8>> {
        let bytes = canonical_json(self)?;
        if bytes.len() > MAX_BATCH_BYTES {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "backup batch exceeds its durable intent budget",
            ));
        }
        Ok(bytes)
    }
}

fn invalid(detail: &str) -> Error {
    Error::new(ErrorCode::ConfigConflict, detail)
}

fn validate_request(request: &BackupPreparationRequest) -> Result<()> {
    Engine::require_actor(&request.principal)?;
    if !(2..=MAX_TARGETS).contains(&request.target_paths.len()) {
        return Err(invalid(
            "batch preparation requires two to eight explicitly selected complete targets",
        ));
    }
    for (i, path) in request.target_paths.iter().enumerate() {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "batch targets require absolute normalized paths",
            ));
        }
        if request.target_paths[..i]
            .iter()
            .any(|other| path.starts_with(other) || other.starts_with(path))
        {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "batch targets overlap; complete copies must be independent",
            ));
        }
    }
    Ok(())
}

fn single_request(request: &BackupPreparationRequest, path: PathBuf) -> BackupPreparationRequest {
    BackupPreparationRequest {
        target_paths: vec![path],
        ..request.clone()
    }
}

pub(super) fn require_no_batch(system: &DiskRoot) -> Result<()> {
    if crate::history_package::preparation_path_exists(system, &system.path().join(BATCH_JOB))? {
        return Err(invalid(
            "an immutable multi-target task is retained; single-target preparation cannot replace it",
        ));
    }
    Ok(())
}

fn require_no_single(system: &DiskRoot) -> Result<()> {
    if crate::history_package::preparation_path_exists(system, &system.path().join(JOB))? {
        return Err(invalid(
            "an immutable single-target task is retained; batch preparation cannot replace it",
        ));
    }
    Ok(())
}

pub(super) fn read_batch(system: &DiskRoot) -> Result<Batch> {
    let batch: Batch =
        decode_strict(&system.read_bytes(&system.path().join(BATCH_JOB), MAX_BATCH_BYTES)?)?;
    batch.validate()?;
    Ok(batch)
}

struct TargetWork {
    root: DiskRoot,
    _lock: AdvisoryLock,
    histories: Vec<PreparedHistoryPackage>,
}

impl Engine {
    /// Persist an explicit set of complete copies as one automatic background
    /// task. The order binds supplied descriptors to request paths. No replica
    /// count, subset assignment, active topology or runtime member is inferred.
    pub fn start_backup_batch_preparation(
        &mut self,
        request: BackupPreparationRequest,
        directories: Vec<File>,
    ) -> Result<BackupPreparationStatus> {
        validate_request(&request)?;
        if directories.len() != request.target_paths.len() {
            return Err(invalid(
                "one held directory is required for every selected batch target",
            ));
        }
        self.require_write()?;
        self.require_unfenced_membership()?;
        self.check_batch_scope(&request)?;
        let system = DiskRoot::open(&self.root)?;
        transaction::require_operation_open(&system, request.operation_id)?;
        require_no_single(&system)?;
        let roots = request
            .target_paths
            .iter()
            .cloned()
            .zip(directories)
            .map(|(path, fd)| DiskRoot::from_directory(path, fd))
            .collect::<Result<Vec<_>>>()?;
        self.check_batch_targets(&roots)?;
        if crate::history_package::preparation_path_exists(&system, &self.root.join(BATCH_JOB))? {
            let retained = read_batch(&system)?;
            if retained.request != request {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "the retained batch binds different targets or source; no task is replaced",
                ));
            }
            for (job, root) in retained.targets.iter().zip(&roots) {
                if PhysicalBinding::of(root)? != job.target {
                    return Err(Error::new(
                        ErrorCode::DiskIdentityMismatch,
                        "selected batch target identity changed",
                    ));
                }
                named_target(root, job)?;
            }
            let running = self
                .backup_preparation
                .as_ref()
                .and_then(|worker| worker.thread.as_ref())
                .is_some_and(|thread| !thread.is_finished());
            if !running {
                if let Some(worker) = self.backup_preparation.as_mut() {
                    worker.join()?;
                }
                self.resume_backup_preparation()?;
            }
            return self
                .backup_preparation_status()
                .ok_or_else(|| invalid("retained batch did not resume"));
        }
        let work = self.batch_work(roots)?;
        let mut jobs = Vec::with_capacity(work.len());
        for (path, target) in request.target_paths.iter().zip(&work) {
            if target
                .root
                .entries(target.root.path())?
                .next()
                .transpose()?
                .is_some()
            {
                return Err(Error::new(
                    ErrorCode::ObjectCollision,
                    "every new batch target must be empty; no partial task was saved",
                ));
            }
            let (history_roots, total_files, total_bytes) = describe(&target.histories)?;
            let job = Job {
                format: super::FORMAT.into(),
                request: single_request(&request, path.clone()),
                pool_id: self.config.pool_id.clone(),
                snapshot: self
                    .snapshot
                    .clone()
                    .ok_or_else(|| invalid("batch preparation requires a published snapshot"))?,
                configuration_hash: object_hash("backup-preparation-config", &self.config)?,
                target: PhysicalBinding::of(&target.root)?,
                history_roots,
                total_files,
                total_bytes,
            };
            named_target(&target.root, &job)?;
            jobs.push(job);
        }
        let batch = Batch {
            format: FORMAT.into(),
            request,
            targets: jobs,
        };
        batch.validate()?;
        let snapshot = snapshot::PreparedSnapshot::freeze(self, batch.first())?;
        // No target writes precede the complete-set resource and identity
        // admission. Capacity is per disk; one target cannot borrow another's.
        for target in &work {
            execution::require_capacity(&target.root, &target.histories, &snapshot)?;
        }
        let bytes = batch.encoded()?;
        system.private_dir(&self.root.join("backup-preparation"))?;
        system.write_atomic(&self.root.join(BATCH_JOB), &bytes, false)?;
        let started = spawn_batch(self, batch.clone(), system, work, snapshot);
        self.backup_preparation = Some(match started {
            Ok(worker) => worker,
            Err(error) => idle(batch, error, BackupPreparationPhase::Blocked),
        });
        Ok(self
            .backup_preparation_status()
            .expect("invariant: retained batch worker"))
    }

    pub(in crate::engine::backup_preparation) fn check_batch_scope(
        &self,
        request: &BackupPreparationRequest,
    ) -> Result<()> {
        self.check_backup_preparation_scope(&single_request(
            request,
            request.target_paths[0].clone(),
        ))?;
        // No absent, disabled, or virtual lower tier is accepted. The complete
        // existing source ancestry must already be admitted; staged copies are
        // not an alternative to offline batch admission of foreign disks.
        let mut slot = request.source_slot_id.as_str();
        loop {
            let disk = self.disk(slot)?;
            if self
                .registered_disk(&disk.disk_id)
                .is_none_or(|d| d.registration_mode != super::super::RegistrationMode::Enabled)
            {
                return Err(invalid(
                    "the complete lower-level source chain must already be admitted",
                ));
            }
            let handle = self.disk_resources.get(disk)?;
            PhysicalBinding::of(&handle.root)?;
            match disk.upstream_slot_id.as_deref() {
                Some(upstream) => slot = upstream,
                None if disk.level == 1 => break,
                None => return Err(invalid("source chain is incomplete")),
            }
        }
        Ok(())
    }

    pub(in crate::engine::backup_preparation) fn check_batch_targets(
        &self,
        roots: &[DiskRoot],
    ) -> Result<()> {
        let mut identities = BTreeSet::new();
        let mut devices = BTreeSet::new();
        if self.config.mode == Mode::Production {
            for disk in &self.config.disks {
                devices.insert(PhysicalBinding::of(&self.disk_resources.get(disk)?.root)?.device);
            }
        }
        for root in roots {
            reject_overlap(self, root)?;
            let binding = PhysicalBinding::of(root)?;
            if !identities.insert((binding.device, binding.inode))
                || (self.config.mode == Mode::Production && !devices.insert(binding.device))
            {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "batch targets must be distinct directories and, in production, separate devices from all source and selected disks",
                ));
            }
        }
        Ok(())
    }

    fn batch_work(&self, roots: Vec<DiskRoot>) -> Result<Vec<TargetWork>> {
        roots
            .into_iter()
            .map(|root| {
                let lock =
                    AdvisoryLock::directory(&root.directory(root.path())?).map_err(fs_error)?;
                Ok(TargetWork {
                    root,
                    _lock: lock,
                    histories: self.backup_history_plans()?,
                })
            })
            .collect()
    }
}

fn idle(batch: Batch, error: Error, phase: BackupPreparationPhase) -> Worker {
    let mut status = batch.status();
    status.phase = phase;
    status.error_code = Some(error.code());
    status.detail = error.to_string();
    Worker::new(batch.first().clone(), Some(batch), status)
}

/// Return whether a retained batch owns preparation. The source binding is
/// checked before any target path is opened; failure never resamples a new job.
pub(super) fn resume(engine: &mut Engine, system: &DiskRoot) -> Result<bool> {
    if !crate::history_package::preparation_path_exists(system, &system.path().join(BATCH_JOB))? {
        return Ok(false);
    }
    require_no_single(system)?;
    let batch = read_batch(system)?;
    if !batch.first().binding_matches(engine)? {
        engine.backup_preparation = Some(idle(
            batch,
            invalid("source changed; original batch is retained without reopening its targets"),
            BackupPreparationPhase::SourceChanged,
        ));
        return Ok(true);
    }
    let result = (|| {
        engine.require_write()?;
        engine.require_unfenced_membership()?;
        engine.check_batch_scope(&batch.request)?;
        let roots = batch
            .request
            .target_paths
            .iter()
            .map(|path| DiskRoot::open(path))
            .collect::<Result<Vec<_>>>()?;
        engine.check_batch_targets(&roots)?;
        for (job, root) in batch.targets.iter().zip(&roots) {
            named_target(root, job)?;
        }
        let work = engine.batch_work(roots)?;
        for (job, target) in batch.targets.iter().zip(&work) {
            if describe(&target.histories)?
                != (job.history_roots.clone(), job.total_files, job.total_bytes)
            {
                return Err(invalid(
                    "a resumed target would omit or change the original whole unit",
                ));
            }
        }
        let snapshot = snapshot::PreparedSnapshot::freeze(engine, batch.first())?;
        let owned_system = DiskRoot::open(system.path())?;
        spawn_batch(engine, batch.clone(), owned_system, work, snapshot)
    })();
    engine.backup_preparation = Some(match result {
        Ok(worker) => worker,
        Err(error) => idle(batch, error, BackupPreparationPhase::Blocked),
    });
    Ok(true)
}

/// This receipt only describes verified material on all explicit targets. It
/// cannot be used as a placement, disk membership, restore or switch token.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BackupPreparationBatchReport {
    pub format: String,
    pub batch_hash: String,
    pub targets: Vec<BackupPreparationReport>,
}

fn completion(
    batch: &Batch,
    reports: Vec<BackupPreparationReport>,
) -> Result<BackupPreparationBatchReport> {
    if reports.len() != batch.targets.len() {
        return Err(invalid("batch completion is missing a target"));
    }
    for (job, report) in batch.targets.iter().zip(&reports) {
        if report.job_hash != object_hash("backup-preparation-job", job)?
            || report.snapshot_hash != batch.first().snapshot
            || report.state_hash != reports[0].state_hash
        {
            return Err(invalid("batch completion binds different source material"));
        }
    }
    Ok(BackupPreparationBatchReport {
        format: FORMAT.into(),
        batch_hash: object_hash("backup-preparation-batch", batch)?,
        targets: reports,
    })
}

/// Check every selected target using target-local evidence only. Supplying one
/// healthy target cannot hide a missing or corrupt peer. This writes nothing.
pub fn offline_check_backup_preparation_batch(
    paths: &[PathBuf],
) -> Result<BackupPreparationBatchReport> {
    if !(2..=MAX_TARGETS).contains(&paths.len()) {
        return Err(invalid("complete batch target set is required"));
    }
    let roots = paths
        .iter()
        .map(|p| DiskRoot::open(p))
        .collect::<Result<Vec<_>>>()?;
    let batch_bytes = roots[0].read_bytes(&roots[0].path().join(BATCH_INTENT), MAX_BATCH_BYTES)?;
    let batch: Batch = decode_strict(&batch_bytes)?;
    batch.validate()?;
    if batch.targets.len() != roots.len() {
        return Err(invalid("batch verifier omits or adds target disks"));
    }
    let stop = AtomicBool::new(false);
    let held = roots.iter().collect::<Vec<_>>();
    let report = verify_targets(&batch, &held, &stop)?;
    let expected = canonical_json(&report)?;
    for (root, job) in roots.iter().zip(&batch.targets) {
        if root.read_bytes(&root.path().join(BATCH_COMPLETE), MAX_BATCH_BYTES)? != expected {
            return Err(invalid(
                "batch completion is absent or belongs to different targets",
            ));
        }
        if PhysicalBinding::of(&DiskRoot::open(root.path())?)? != job.target {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "batch target name changed during inspection",
            ));
        }
    }
    Ok(report)
}

pub(super) fn verify_targets(
    batch: &Batch,
    roots: &[&DiskRoot],
    stop: &AtomicBool,
) -> Result<BackupPreparationBatchReport> {
    let manifest = batch.encoded()?;
    let mut reports = Vec::with_capacity(roots.len());
    for (root, job) in roots.iter().zip(&batch.targets) {
        crate::history_package::preparation_checkpoint(stop)?;
        if PhysicalBinding::of(root)? != job.target
            || root.read_bytes(&root.path().join(BATCH_INTENT), MAX_BATCH_BYTES)? != manifest
        {
            return Err(invalid("target identity or complete-set intent differs"));
        }
        let report = inspection::check_at(root, stop)?;
        reports.push(report);
    }
    completion(batch, reports)
}

fn spawn_batch(
    engine: &Engine,
    batch: Batch,
    system: DiskRoot,
    mut work: Vec<TargetWork>,
    snapshot: snapshot::PreparedSnapshot,
) -> Result<Worker> {
    let locks = engine.retain_writer_locks()?;
    let worker = Worker::new(batch.first().clone(), Some(batch.clone()), batch.status());
    worker.spawn(locks, system, "zxs-backup-batch", move |progress| {
        // All target locks and source body leases outlive the entire batch.
        let update = |action: &dyn Fn(&mut BackupPreparationStatus)| progress.update(action);
        let cancel = progress.stop.as_ref();
        let result = (|| {
            let intent = batch.encoded()?;
            for (job, target) in batch.targets.iter().zip(&work) {
                named_target(&target.root, job)?;
                execution::require_capacity(&target.root, &target.histories, &snapshot)?;
            }
            for (index, (job, target)) in batch.targets.iter().zip(&mut work).enumerate() {
                crate::history_package::preparation_checkpoint(cancel)?;
                update(&|status| {
                    status.detail = format!(
                        "target {}/{}: preparing complete frozen unit at {}; placement unchanged",
                        index + 1,
                        batch.targets.len(),
                        target.root.path().display(),
                    );
                })?;
                let copied = (|| {
                    reserve_target(&target.root, job)?;
                    crate::history_package::install_preparation_bytes(
                        &target.root,
                        &target.root.path().join(BATCH_INTENT),
                        &intent,
                    )?;
                    execution::copy_target(
                        job,
                        &target.root,
                        std::mem::take(&mut target.histories),
                        &snapshot,
                        cancel,
                        &update,
                    )
                })();
                copied.map_err(|error| {
                    let code = error.code();
                    error.context(
                        code,
                        format!(
                            "batch target {} failed; other copies and all source data retained",
                            index + 1,
                        ),
                    )
                })?;
            }
            update(&|status| {
                status.phase = BackupPreparationPhase::Verifying;
                status.detail = "revalidating every complete target before the batch material marker".into();
            })?;
            let roots = work.iter().map(|w| &w.root).collect::<Vec<_>>();
            let report = verify_targets(&batch, &roots, cancel)?;
            let bytes = canonical_json(&report)?;
            if bytes.len() > MAX_BATCH_BYTES {
                return Err(Error::new(
                    ErrorCode::ResourceLimit,
                    "batch completion exceeds reader budget",
                ));
            }
            for (root, job) in roots.iter().zip(&batch.targets) {
                named_target(root, job)?;
                crate::history_package::preparation_checkpoint(cancel)?;
                crate::history_package::install_preparation_bytes(
                    root,
                    &root.path().join(BATCH_COMPLETE),
                    &bytes,
                )?;
            }
            for (root, job) in roots.iter().zip(&batch.targets) {
                named_target(root, job)?;
            }
            update(&|status| {
                status.phase = BackupPreparationPhase::HistoryPrepared;
                status.detail = format!(
                    "all {} explicit targets contain complete frozen State and history; database installation, chain admission and authority switch remain unsupported",
                    batch.targets.len(),
                );
            })
        })();
        progress.finish(result);
    })
}
