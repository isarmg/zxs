//! The SYSTEM extension is durable before target writes. A changed source stops
//! before opening old target paths; a partial archive retries only the same task.
use super::*;
use archive::PreparedArchive;

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
            let work = prepare(self, &selection)?;
            let archive = PreparedArchive::freeze(self, selection.first())?;
            for target in &work {
                require_capacity(&target.root, &archive)?;
            }
            let task = NativeTask {
                format: FORMAT.into(),
                selection,
                archive_hash: archive.report.archive_hash.clone(),
            };
            // Immutable owner-side intent precedes even a target intent. A
            // partially persisted attempt can never silently select other disks.
            system.write_atomic(&self.root.join(TASK), &task.bytes()?, false)?;
            self.backup_preparation =
                Some(match spawn(self, task.clone(), system, work, archive) {
                    Ok(worker) => worker,
                    Err(error) => task.selection.idle(error, BackupPreparationPhase::Blocked),
                });
        }
        self.backup_preparation_status()
            .ok_or_else(|| invalid("native task did not retain an observable worker"))
    }
}

pub(super) fn prepare(engine: &Engine, selection: &Selection) -> Result<Vec<Target>> {
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
    selection.verify_material(
        &targets
            .iter()
            .map(|target| &target.root)
            .collect::<Vec<_>>(),
        &AtomicBool::new(false),
    )?;
    Ok(targets)
}

fn require_capacity(root: &DiskRoot, archive: &PreparedArchive) -> Result<()> {
    let needed = archive
        .remaining_bytes(root)?
        .checked_add(RESERVE)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "native preparation capacity overflow",
            )
        })?;
    if root.available()? < needed {
        return Err(Error::new(
            ErrorCode::NoSpace,
            "one selected target cannot retain its entire original native prefix",
        ));
    }
    Ok(())
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
        let work = prepare(engine, &task.selection)?;
        let archive = PreparedArchive::freeze(engine, task.selection.first())?;
        if archive.report.archive_hash != task.archive_hash {
            return Err(invalid(
                "original native prefix changed; retained bytes were not overwritten",
            ));
        }
        for target in &work {
            require_capacity(&target.root, &archive)?;
        }
        spawn(
            engine,
            task.clone(),
            DiskRoot::open(system.path())?,
            work,
            archive,
        )
    })();
    engine.backup_preparation = Some(match resumed {
        Ok(worker) => worker,
        Err(error) => task.selection.idle(error, BackupPreparationPhase::Blocked),
    });
    Ok(true)
}

fn spawn(
    engine: &Engine,
    task: NativeTask,
    system: DiskRoot,
    work: Vec<Target>,
    archive: PreparedArchive,
) -> Result<Worker> {
    let locks = engine.retain_writer_locks()?;
    let state = Arc::new(Mutex::new(task.selection.status()));
    let stop = Arc::new(AtomicBool::new(false));
    let progress = Arc::clone(&state);
    let cancellation = Arc::clone(&stop);
    let frozen = task.clone();
    let thread = std::thread::Builder::new().name("zxs-backup-native-control".into()).spawn(move || {
        let _locks = locks;
        // All targets remain held and locked through the last complete-set
        // marker. No diagnostic mutex is held while copying or syncing bytes.
        let update = |action: &dyn Fn(&mut BackupPreparationStatus)| -> Result<()> {
            let mut status = progress.lock().expect("invariant: native progress poisoned").clone();
            action(&mut status);
            let persisted = system.write_json(&system.path().join(PROGRESS), &status, true);
            *progress.lock().expect("invariant: native progress poisoned") = status;
            persisted
        };
        let result = (|| {
            let intent = frozen.bytes()?;
            for (target, job) in work.iter().zip(frozen.selection.jobs()) {
                named_target(&target.root, job)?;
                require_capacity(&target.root, &archive)?;
            }
            for (index, (target, job)) in work.iter().zip(frozen.selection.jobs()).enumerate() {
                crate::history_package::preparation_checkpoint(&cancellation)?;
                update(&|status| { status.phase = BackupPreparationPhase::Verifying; status.detail = format!("native evidence target {}/{}; original member journals and publication closure, no authority switch", index + 1, work.len()); })?;
                named_target(&target.root, job)?;
                crate::history_package::install_preparation_bytes(&target.root, &target.root.path().join(INTENT), &intent)?;
                archive.copy(&target.root, &cancellation)?;
                let report = archive::read(&target.root, job, &frozen.archive_hash, &cancellation)?;
                if report != archive.report { return Err(invalid("target native prefix differs from the independently frozen original")); }
            }
            let roots = work.iter().map(|target| &target.root).collect::<Vec<_>>();
            let report = verify_targets(&frozen, &roots, &cancellation)?;
            let bytes = canonical_json(&report)?;
            if bytes.len() > MAX_TASK { return Err(Error::new(ErrorCode::ResourceLimit, "native completion exceeds reader budget")); }
            for (target, job) in work.iter().zip(frozen.selection.jobs()) {
                crate::history_package::preparation_checkpoint(&cancellation)?;
                named_target(&target.root, job)?;
                crate::history_package::install_preparation_bytes(&target.root, &target.root.path().join(COMPLETE), &bytes)?;
            }
            for (target, job) in work.iter().zip(frozen.selection.jobs()) { named_target(&target.root, job)?; }
            update(&|status| { status.phase = BackupPreparationPhase::NativeControlPrepared; status.detail = format!("all {} targets independently retain original native control and complete history material; database installation, complete-chain admission and cutover are not implemented", work.len()); })
        })();
        if let Err(error) = result {
            if let Err(persistence) = update(&|status| { status.phase = BackupPreparationPhase::Blocked; status.error_code = Some(error.code()); status.detail = error.to_string(); }) {
                progress.lock().expect("invariant: native progress poisoned").detail = format!("{error}; progress persistence failed: {persistence}");
            }
        }
    })?;
    let job = task.selection.first().clone();
    let batch = match task.selection {
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
