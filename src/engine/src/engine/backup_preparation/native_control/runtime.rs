//! The SYSTEM extension is durable before target writes. A changed source stops
//! before opening old target paths; a partial archive retries only the same task.
use super::*;
use allocation::Admission;
use archive::PreparedArchive;
use completion_fanout::{Fanout, MaterialMode};

pub(super) struct Target {
    pub(super) root: DiskRoot,
    _lock: AdvisoryLock,
}

impl Engine {
    /// Add complete original fixed-genesis native control evidence to the exact
    /// retained history task. The same caller/operation and whole target set are
    /// required; no new copy policy, registration or final switch is inferred.
    /// Hosts must authenticate the caller before invoking this trusted API.
    pub fn start_backup_native_preparation(
        &mut self,
        operation_id: OperationId,
        principal: &str,
    ) -> Result<BackupPreparationStatus> {
        Self::require_actor(principal)?;
        self.require_write()?;
        self.require_unfenced_membership()?;
        let system = DiskRoot::open(&self.root)?;
        let selection = Selection::read(&system)?;
        if selection.first().request.operation_id != operation_id
            || selection.first().request.principal != principal
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "native preparation must belong to the exact original actor and operation",
            ));
        }
        if self
            .backup_preparation
            .as_ref()
            .and_then(|worker| worker.thread.as_ref())
            .is_some_and(|thread| !thread.is_finished())
        {
            return Err(Error::new(
                ErrorCode::WorkspaceBusy,
                "wait for the retained background stage; no parallel native worker was started",
            ));
        }
        if let Some(worker) = self.backup_preparation.as_mut() {
            worker.join()?;
        }
        if crate::history_package::preparation_path_exists(&system, &self.root.join(TASK))? {
            super::resume(self, &system)?;
        } else {
            let (work, materials) = prepare_with_reports(self, &selection)?;
            let fanout = Fanout::observe(COMPLETE, &work, selection.jobs())?;
            if matches!(fanout.mode(), MaterialMode::ExistingOnly) {
                return Err(invalid("native completion predates the SYSTEM intent"));
            }
            for target in &work {
                for name in [INTENT, archive::ROOT, COMPLETE] {
                    if target.root.optional_metadata(&target.root.path().join(name))?.is_some() {
                        return Err(invalid("native target evidence predates its SYSTEM intent; existing material retained"));
                    }
                }
            }
            let archive = PreparedArchive::freeze(self, selection.first())?;
            let task = NativeTask {
                format: FORMAT.into(),
                selection,
                archive_hash: archive.report.archive_hash.clone(),
            };
            // Immutable owner-side intent precedes even a target intent. A
            // partially persisted attempt can never silently select other disks.
            let admission = Admission::capture(&task, &archive, &fanout, materials)?;
            admission.require_material(&work, &task, &archive, fanout.mode(), &AtomicBool::new(false))?;
            admission.require_new_system_intent(&system)?;
            system.write_atomic(&self.root.join(TASK), admission.intent(), false)?;
            self.backup_preparation =
                Some(match spawn(self, task.clone(), system, work, archive, fanout, admission) {
                    Ok(worker) => worker,
                    Err(error) => task.selection.idle(error, BackupPreparationPhase::Blocked),
                });
        }
        self.backup_preparation_status()
            .ok_or_else(|| invalid("native task did not retain an observable worker"))
    }
}

pub(super) fn prepare(engine: &Engine, selection: &Selection) -> Result<Vec<Target>> {
    prepare_with_reports(engine, selection).map(|(targets, _)| targets)
}

fn prepare_with_reports(engine: &Engine, selection: &Selection) -> Result<(Vec<Target>, Vec<BackupPreparationReport>)> {
    if !selection.first().binding_matches(engine)? {
        return Err(invalid(
            "native source changed; original task cannot be resampled",
        ));
    }
    engine.require_write()?;
    engine.require_unfenced_membership()?;
    if engine.config.generation().is_some() || engine.current_namespaces.is_some() {
        return Err(Error::new(
            ErrorCode::UnsupportedOperation,
            "native preparation requires a complete fixed-genesis authority; generation control closure remains unsupported",
        ));
    }
    // Existing whole-unit admission remains in force, including the complete
    // source ancestry for a batch. This does not admit any foreign placeholder.
    match selection {
        Selection::Single(job) => engine.check_batch_scope(&job.request)?,
        Selection::Batch(batch) => engine.check_batch_scope(batch.request())?,
    }
    let roots = selection
        .jobs()
        .iter()
        .map(|job| DiskRoot::open(&job.request.target_paths[0]))
        .collect::<Result<Vec<_>>>()?;
    engine.check_batch_targets(&roots)?;
    let mut targets = Vec::with_capacity(roots.len());
    for (root, job) in roots.into_iter().zip(selection.jobs()) {
        if PhysicalBinding::of(&root)? != job.target {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "native preparation target binding changed",
            ));
        }
        named_target(&root, job)?;
        let lock = AdvisoryLock::directory(&root.directory(root.path())?).map_err(fs_error)?;
        targets.push(Target { root, _lock: lock });
    }
    let reports = selection.verify_material(
        &targets
            .iter()
            .map(|target| &target.root)
            .collect::<Vec<_>>(),
        &AtomicBool::new(false),
    )?;
    Ok((targets, reports))
}

pub(in crate::engine::backup_preparation) fn resume(
    engine: &mut Engine,
    system: &DiskRoot,
) -> Result<bool> {
    if !crate::history_package::preparation_path_exists(system, &system.path().join(TASK))? {
        return Ok(false);
    }
    let task = NativeTask::read(system)?;
    if !task.selection.first().binding_matches(engine)? {
        engine.backup_preparation = Some(task.selection.idle(
            invalid("native source boundary changed; no target paths reopened"),
            BackupPreparationPhase::SourceChanged,
        ));
        return Ok(true);
    }
    let resumed = (|| {
        let (work, materials) = prepare_with_reports(engine, &task.selection)?;
        let archive = PreparedArchive::freeze(engine, task.selection.first())?;
        if archive.report.archive_hash != task.archive_hash {
            return Err(invalid(
                "original native prefix changed; retained bytes were not overwritten",
            ));
        }
        let fanout = Fanout::observe(COMPLETE, &work, task.selection.jobs())?;
        let admission = Admission::capture(&task, &archive, &fanout, materials)?;
        admission.require_material(&work, &task, &archive, fanout.mode(), &AtomicBool::new(false))?;
        spawn(
            engine,
            task.clone(),
            DiskRoot::open(system.path())?,
            work,
            archive,
            fanout,
            admission,
        )
    })();
    engine.backup_preparation = Some(match resumed {
        Ok(worker) => worker,
        Err(error) => task.selection.idle(error, BackupPreparationPhase::Blocked),
    });
    Ok(true)
}

fn copy_control(
    task: &NativeTask,
    work: &[Target],
    archive: &PreparedArchive,
    mut fanout: Fanout,
    admission: &Admission,
    progress: &Progress,
) -> Result<()> {
    let stop = progress.stop.as_ref();
    let intent = admission.intent();
    fanout.refresh(work, task.selection.jobs())?;
    let mode = fanout.mode();
    admission.require_material(work, task, archive, mode, stop)?;
    for (index, (target, job)) in work.iter().zip(task.selection.jobs()).enumerate() {
        crate::history_package::preparation_checkpoint(stop)?;
        progress.update(|status| {
            status.phase = BackupPreparationPhase::Verifying;
            status.detail = format!(
                "native evidence target {}/{}; original member journals and publication closure, no authority switch",
                index + 1,
                work.len(),
            );
        })?;
        named_target(&target.root, job)?;
        mode.write(&target.root, &target.root.path().join(INTENT), intent)?;
        archive.install(&target.root, mode, stop)?;
        let report = archive::read(&target.root, job, &task.archive_hash, stop)?;
        if report != archive.report {
            return Err(invalid(
                "target native prefix differs from the independently frozen original",
            ));
        }
    }
    let roots = work.iter().map(|target| &target.root).collect::<Vec<_>>();
    let report = verify_targets(task, &roots, stop)?;
    let bytes = canonical_json(&report)?;
    if bytes.len() > MAX_TASK {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "native completion exceeds reader budget",
        ));
    }
    fanout.finish(work, task.selection.jobs(), &bytes, MAX_TASK, stop)?;
    progress.update(|status| {
        status.phase = BackupPreparationPhase::NativeControlPrepared;
        status.detail = format!(
            "all {} targets independently retain original native control and complete history material; database installation, complete-chain admission and cutover are not implemented",
            work.len(),
        );
    })
}

fn spawn(
    engine: &Engine,
    task: NativeTask,
    system: DiskRoot,
    work: Vec<Target>,
    archive: PreparedArchive,
    fanout: Fanout,
    admission: Admission,
) -> Result<Worker> {
    let locks = engine.retain_writer_locks()?;
    let initial = task.selection.status();
    let frozen = task.clone();
    task.selection.worker(initial).spawn(
        locks,
        system,
        "zxs-backup-native-control",
        move |progress| {
            // Retain all target locks through complete-set and failure diagnostics.
            progress.finish(copy_control(&frozen, &work, &archive, fanout, &admission, progress));
        },
    )
}
