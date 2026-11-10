//! Durable next stage of the same target selection. No new placement, member
//! identity, source deletion or business publication is created by this worker.
use super::*;
use crate::engine::backup_preparation::native_control::runtime::Target;
use crate::history_package::{
    install_preparation_bytes, preparation_checkpoint, preparation_path_exists,
};

impl Engine {
    /// Prepare the existing SYSTEM-current and original disk-current/history
    /// SQLite formats on every selected target. These are source-scoped
    /// materials, not complete three-scope history or replacement authority.
    pub fn start_backup_scoped_database_preparation(
        &mut self,
        operation_id: OperationId,
        principal: &str,
    ) -> Result<BackupPreparationStatus> {
        Self::require_actor(principal)?;
        self.require_write()?;
        self.require_unfenced_membership()?;
        let system = DiskRoot::open(&self.root)?;
        let objects = DatabaseTask::read(&system)?;
        let request = &objects.native.selection.first().request;
        if request.operation_id != operation_id || request.principal != principal {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "scoped stage belongs to another original task or actor",
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
                "wait for the retained worker before extending its original preparation",
            ));
        }
        if let Some(worker) = self.backup_preparation.as_mut() {
            worker.join()?;
        }
        if preparation_path_exists(&system, &self.root.join(TASK))? {
            resume(self, &system)?;
        } else {
            let task = ScopedTask {
                format: FORMAT.into(),
                objects,
            };
            let (work, objects) = prepare(self, &task)?;
            // Owner first, every target second. Existing exact intent can be
            // resumed, never replaced by a different selected peer set.
            system.write_atomic(&self.root.join(TASK), &task.bytes()?, false)?;
            self.backup_preparation =
                Some(match spawn(self, task.clone(), system, work, objects) {
                    Ok(worker) => worker,
                    Err(error) => task
                        .objects
                        .native
                        .selection
                        .idle(error, BackupPreparationPhase::Blocked),
                });
        }
        self.backup_preparation_status()
            .ok_or_else(|| invalid("scoped task has no observable retained worker"))
    }
}

fn prepare(
    engine: &Engine,
    task: &ScopedTask,
) -> Result<(Vec<Target>, BackupObjectDatabasePreparationReport)> {
    // Shared target preparation checks the full live source binding before
    // opening any old target path, and owns every target's original lock.
    let work = super::super::super::runtime::prepare(engine, &task.objects.native.selection)?;
    let objects = super::super::verify_at(
        &task.objects,
        &work.iter().map(|target| &target.root).collect::<Vec<_>>(),
        &AtomicBool::new(false),
    )?;
    Ok((work, objects))
}

pub(in crate::engine::backup_preparation) fn resume(
    engine: &mut Engine,
    system: &DiskRoot,
) -> Result<bool> {
    if super::cutover::resume(engine, system)? {
        return Ok(true);
    }
    if !preparation_path_exists(system, &system.path().join(TASK))? {
        return Ok(false);
    }
    let task = ScopedTask::read(system)?;
    if !task
        .objects
        .native
        .selection
        .first()
        .binding_matches(engine)?
    {
        engine.backup_preparation = Some(task.objects.native.selection.idle(
            invalid("scoped source changed; no old target path was reopened"),
            BackupPreparationPhase::SourceChanged,
        ));
        return Ok(true);
    }
    let result = (|| {
        let (work, objects) = prepare(engine, &task)?;
        spawn(
            engine,
            task.clone(),
            DiskRoot::open(system.path())?,
            work,
            objects,
        )
    })();
    engine.backup_preparation = Some(match result {
        Ok(worker) => worker,
        Err(error) => task
            .objects
            .native
            .selection
            .idle(error, BackupPreparationPhase::Blocked),
    });
    Ok(true)
}

fn run(
    task: &ScopedTask,
    work: &[Target],
    objects: BackupObjectDatabasePreparationReport,
    progress: &Progress,
) -> Result<()> {
    let stop = progress.stop.as_ref();
    preparation_checkpoint(stop)?;
    let prepared = PreparedScopes::capture(
        &work[0].root,
        task,
        task.objects.native.selection.first(),
        stop,
    )?;
    let expected = completion_bytes(&report(task, objects, &prepared)?)?;
    let intent = task.bytes()?;
    for (target, job) in work.iter().zip(task.objects.native.selection.jobs()) {
        preparation_checkpoint(stop)?;
        named_target(&target.root, job)?;
        store::require_capacity(&target.root, &prepared, stop)?;
    }
    for (index, (target, job)) in work
        .iter()
        .zip(task.objects.native.selection.jobs())
        .enumerate()
    {
        preparation_checkpoint(stop)?;
        progress.update(|status| {
            status.phase = BackupPreparationPhase::Verifying;
            status.detail = format!(
                "original-scope SQLite target {}/{}; not an authority cutover",
                index + 1,
                work.len()
            );
        })?;
        named_target(&target.root, job)?;
        install_preparation_bytes(&target.root, &target.root.path().join(INTENT), &intent)?;
        store::install(&target.root, &prepared, stop)?;
    }
    let counts = (prepared.disk_count(), prepared.databases().len());
    drop(prepared);
    // After writes, all previous stages and bodies are re-read. Then every
    // peer supplies its own prefix to derive the expected database bytes;
    // keeping the first healthy peer's projection is not verification.
    let objects = super::super::verify_at(
        &task.objects,
        &work.iter().map(|target| &target.root).collect::<Vec<_>>(),
        stop,
    )?;
    for (target, job) in work.iter().zip(task.objects.native.selection.jobs()) {
        preparation_checkpoint(stop)?;
        named_target(&target.root, job)?;
        let local = PreparedScopes::capture(&target.root, task, job, stop)?;
        store::verify(&target.root, &local, stop)?;
        if completion_bytes(&report(task, objects.clone(), &local)?)? != expected {
            return Err(invalid(
                "selected targets have different source-scoped database closures",
            ));
        }
    }
    for (target, job) in work.iter().zip(task.objects.native.selection.jobs()) {
        preparation_checkpoint(stop)?;
        named_target(&target.root, job)?;
        install_preparation_bytes(&target.root, &target.root.path().join(COMPLETE), &expected)?;
    }
    for (target, job) in work.iter().zip(task.objects.native.selection.jobs()) {
        named_target(&target.root, job)?;
    }
    progress.update(|status| {
        status.phase = BackupPreparationPhase::ScopedDatabasesPrepared;
        status.detail = format!(
            "all {} targets retain {} original disk scopes / {} scoped SQLite files each; complete audit, active installation and authority switch remain unsupported",
            work.len(),
            counts.0,
            counts.1
        );
    })
}

fn spawn(
    engine: &Engine,
    task: ScopedTask,
    system: DiskRoot,
    work: Vec<Target>,
    objects: BackupObjectDatabasePreparationReport,
) -> Result<Worker> {
    let locks = engine.retain_writer_locks()?;
    let mut initial = task.objects.native.selection.status();
    initial.detail =
        "preparing original SYSTEM/disk SQLite scopes; active memberships unchanged".into();
    let frozen = task.clone();
    task.objects.native.selection.worker(initial).spawn(
        locks,
        system,
        "zxs-backup-scoped-databases",
        move |progress| {
            progress.finish(run(&frozen, &work, objects, progress));
        },
    )
}
