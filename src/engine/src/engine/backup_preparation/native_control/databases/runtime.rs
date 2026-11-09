//! A third, explicit preparation stage. Its immutable owner intent precedes
//! target writes; startup resumes it before either earlier worker stage.
use super::super::runtime::Target;
use super::*;
use crate::history_package::{
    install_preparation_bytes, preparation_checkpoint, preparation_path_exists,
};

impl Engine {
    /// Prepare FILE and directory-repository SQLite on every original target.
    /// This does not prepare the SYSTEM/disk historical scopes, register a
    /// foreign chain, accept a journal Candidate or change an active placement.
    pub fn start_backup_object_database_preparation(
        &mut self,
        operation_id: OperationId,
        principal: &str,
    ) -> Result<BackupPreparationStatus> {
        Self::require_actor(principal)?;
        self.require_write()?;
        self.require_unfenced_membership()?;
        let system = DiskRoot::open(&self.root)?;
        let native = NativeTask::read(&system)?;
        if native.selection.first().request.operation_id != operation_id
            || native.selection.first().request.principal != principal
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "object database stage belongs to another original operation or actor",
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
                "wait for the current retained preparation; no concurrent stage was started",
            ));
        }
        if let Some(worker) = self.backup_preparation.as_mut() {
            worker.join()?;
        }
        if preparation_path_exists(&system, &self.root.join(TASK))? {
            resume(self, &system)?;
        } else {
            let (work, original) = prepare(self, &native)?;
            let task = DatabaseTask {
                format: FORMAT.into(),
                state_hash: original.targets[0].material.state_hash.clone(),
                native,
            };
            task.check_native(&original)?;
            system.write_atomic(&self.root.join(TASK), &task.bytes()?, false)?;
            self.backup_preparation =
                Some(match spawn(self, task.clone(), system, work, original) {
                    Ok(worker) => worker,
                    Err(error) => task
                        .native
                        .selection
                        .idle(error, BackupPreparationPhase::Blocked),
                });
        }
        self.backup_preparation_status()
            .ok_or_else(|| invalid("database task has no observable worker"))
    }
}

fn prepare(
    engine: &Engine,
    native: &NativeTask,
) -> Result<(Vec<Target>, BackupNativePreparationReport)> {
    // Binding is checked before root names are reopened by the shared prepare.
    let work = super::super::runtime::prepare(engine, &native.selection)?;
    let roots = work.iter().map(|target| &target.root).collect::<Vec<_>>();
    let report = verify_completed(native, &roots, &AtomicBool::new(false))?;
    Ok((work, report))
}

pub(in crate::engine::backup_preparation) fn resume(
    engine: &mut Engine,
    system: &DiskRoot,
) -> Result<bool> {
    if scoped::resume(engine, system)? {
        return Ok(true);
    }
    if !preparation_path_exists(system, &system.path().join(TASK))? {
        return Ok(false);
    }
    let task = DatabaseTask::read(system)?;
    if !task.native.selection.first().binding_matches(engine)? {
        engine.backup_preparation = Some(task.native.selection.idle(
            invalid("object database source changed; old target names were not reopened"),
            BackupPreparationPhase::SourceChanged,
        ));
        return Ok(true);
    }
    let result = (|| {
        let (work, report) = prepare(engine, &task.native)?;
        task.check_native(&report)?;
        spawn(
            engine,
            task.clone(),
            DiskRoot::open(system.path())?,
            work,
            report,
        )
    })();
    engine.backup_preparation = Some(match result {
        Ok(worker) => worker,
        Err(error) => task
            .native
            .selection
            .idle(error, BackupPreparationPhase::Blocked),
    });
    Ok(true)
}

pub(super) fn update(
    system: &DiskRoot,
    progress: &Mutex<BackupPreparationStatus>,
    action: impl FnOnce(&mut BackupPreparationStatus),
) -> Result<()> {
    let mut status = progress
        .lock()
        .expect("invariant: database progress poisoned")
        .clone();
    action(&mut status);
    let persisted = system.write_json(&system.path().join(PROGRESS), &status, true);
    *progress
        .lock()
        .expect("invariant: database progress poisoned") = status;
    persisted
}

fn run(
    task: &DatabaseTask,
    system: &DiskRoot,
    work: &[Target],
    native: BackupNativePreparationReport,
    progress: &Mutex<BackupPreparationStatus>,
    stop: &AtomicBool,
) -> Result<()> {
    preparation_checkpoint(stop)?;
    let state = snapshot::read(&work[0].root, task.native.selection.first(), stop)?;
    if object_hash("backup-prepared-state", state.state())? != task.state_hash {
        return Err(invalid(
            "target State changed before object database projection",
        ));
    }
    let prepared = PreparedDatabases::capture(&state, stop)?;
    let intent = task.bytes()?;
    let observation = report(task, native, &prepared)?;
    let bytes = completion_bytes(&observation)?;
    // All targets pass their own partial inventory/space checks before the
    // first target receives even this stage's immutable intent.
    for (target, job) in work.iter().zip(task.native.selection.jobs()) {
        preparation_checkpoint(stop)?;
        named_target(&target.root, job)?;
        store::require_capacity(&target.root, &prepared, stop)?;
    }
    for (index, (target, job)) in work.iter().zip(task.native.selection.jobs()).enumerate() {
        preparation_checkpoint(stop)?;
        update(system, progress, |status| {
            status.phase = BackupPreparationPhase::Verifying;
            status.detail = format!(
                "object SQLite target {}/{}; complete files and nested directory repositories, no authority switch",
                index + 1,
                work.len()
            );
        })?;
        named_target(&target.root, job)?;
        install_preparation_bytes(&target.root, &target.root.path().join(INTENT), &intent)?;
        store::install(&target.root, &prepared, stop)?;
    }
    // Re-read the original control prefix and every retained FULL after all
    // SQLite writes, before any common completion marker. A good database is
    // never a substitute for a missing body, old receipt or changed target.
    let roots = work.iter().map(|target| &target.root).collect::<Vec<_>>();
    let final_native = verify_completed(&task.native, &roots, stop)?;
    if completion_bytes(&report(task, final_native, &prepared)?)? != bytes {
        return Err(invalid(
            "native material changed during database preparation",
        ));
    }
    for (target, job) in work.iter().zip(task.native.selection.jobs()) {
        preparation_checkpoint(stop)?;
        named_target(&target.root, job)?;
        store::verify(&target.root, &prepared, stop)?;
    }
    for (target, job) in work.iter().zip(task.native.selection.jobs()) {
        preparation_checkpoint(stop)?;
        named_target(&target.root, job)?;
        install_preparation_bytes(&target.root, &target.root.path().join(COMPLETE), &bytes)?;
    }
    for (target, job) in work.iter().zip(task.native.selection.jobs()) {
        named_target(&target.root, job)?;
    }
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::ObjectDatabasesPrepared;
        status.detail = format!(
            "all {} targets retain {} file and {} directory databases each; SYSTEM/disk historical databases, complete-chain admission and cutover remain unsupported",
            work.len(),
            prepared.file_count(),
            prepared.directory_count()
        );
    })
}

fn spawn(
    engine: &Engine,
    task: DatabaseTask,
    system: DiskRoot,
    work: Vec<Target>,
    native: BackupNativePreparationReport,
) -> Result<Worker> {
    let locks = engine.retain_writer_locks()?;
    let mut initial = task.native.selection.status();
    initial.detail =
        "preparing all object databases from complete admitted State; active disks unchanged"
            .into();
    let state = Arc::new(Mutex::new(initial));
    let stop = Arc::new(AtomicBool::new(false));
    let progress = Arc::clone(&state);
    let cancellation = Arc::clone(&stop);
    let frozen = task.clone();
    let thread = std::thread::Builder::new()
        .name("zxs-backup-object-databases".into())
        .spawn(move || {
            let _locks = locks;
            if let Err(error) = run(&frozen, &system, &work, native, &progress, &cancellation)
                && let Err(persistence) = update(&system, &progress, |status| {
                    status.phase = BackupPreparationPhase::Blocked;
                    status.error_code = Some(error.code());
                    status.detail = error.to_string();
                })
            {
                progress
                    .lock()
                    .expect("invariant: database progress poisoned")
                    .detail = format!("{error}; progress persistence failed: {persistence}");
            }
        })?;
    let job = task.native.selection.first().clone();
    let batch = match task.native.selection {
        Selection::Single(_) => None,
        Selection::Batch(batch) => Some(batch),
    };
    Ok(Worker {
        job,
        batch,
        state,
        stop,
        thread: Some(thread),
        targets: Vec::new(),
    })
}
