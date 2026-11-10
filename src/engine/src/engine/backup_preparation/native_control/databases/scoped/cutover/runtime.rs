//! Persist owner intent before per-target topology input acknowledgements.
//! There is no membership vote or write-fence release in this preparation step.
use super::*;
use crate::engine::backup_preparation::native_control::runtime::Target;

impl Engine {
    /// Freeze explicit new identities and all lower-level edges for the original
    /// whole-unit task, then independently verify and durably acknowledge the
    /// inputs on every target in the background. A full subtree is one request.
    ///
    /// This does not activate replacement disks or select new metadata voters.
    /// Active Config remains on its historical topology until a separate native
    /// authority/placement decision and complete database-history protocol exist.
    pub fn start_backup_cutover_preparation(
        &mut self,
        request: BackupCutoverPreparationRequest,
    ) -> Result<BackupPreparationStatus> {
        Self::require_actor(&request.principal)?;
        self.require_write()?;
        self.require_unfenced_membership()?;
        let system = DiskRoot::open(&self.root)?;
        let scoped = ScopedTask::read(&system)?;
        let original = &scoped.objects.native.selection.first().request;
        if request.operation_id != original.operation_id || request.principal != original.principal
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "cutover preparation must retain the original actor and operation",
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
                "the original backup worker must finish before extending its input set",
            ));
        }
        if let Some(worker) = self.backup_preparation.as_mut() {
            worker.join()?;
        }
        if preparation_path_exists(&system, &self.root.join(TASK))? {
            let retained = CutoverTask::read(&system)?;
            if retained.request != request {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "the immutable cutover task names different prospective bindings or edges",
                ));
            }
            resume(self, &system)?;
        } else {
            let topology = Topology::derive(
                &self.config,
                &scoped.objects.native.selection,
                &request.targets,
            )?;
            let task = CutoverTask {
                format: FORMAT.into(),
                scoped,
                request,
                original: (*self.config).clone(),
                topology,
            };
            task.validate()?;
            let (work, report) = prepare(self, &task)?;
            let bytes = task.bytes()?;
            for (index, target) in work.iter().enumerate() {
                require_capacity(&target.root, &task, &report, index)?;
            }
            system.write_atomic(&self.root.join(TASK), &bytes, false)?;
            self.backup_preparation = Some(match spawn(self, task.clone(), system, work, report) {
                Ok(worker) => worker,
                Err(error) => task
                    .selection()
                    .clone()
                    .idle(error, BackupPreparationPhase::Blocked),
            });
        }
        self.backup_preparation_status()
            .ok_or_else(|| invalid("cutover task has no retained worker"))
    }
}

fn prepare(
    engine: &Engine,
    task: &CutoverTask,
) -> Result<(Vec<Target>, BackupCutoverPreparationReport)> {
    // Checks exact live source before opening any retained target pathname,
    // complete enabled original ancestry, physical separation and every lock.
    let work = super::super::super::super::runtime::prepare(engine, task.selection())?;
    let roots = work.iter().map(|target| &target.root).collect::<Vec<_>>();
    let stop = AtomicBool::new(false);
    let scoped = super::super::verify_at(&task.scoped, &roots, &stop)?;
    let report = report(task, scoped, &roots, &stop)?;
    Ok((work, report))
}

pub(in crate::engine::backup_preparation) fn resume(
    engine: &mut Engine,
    system: &DiskRoot,
) -> Result<bool> {
    if !preparation_path_exists(system, &system.path().join(TASK))? {
        return Ok(false);
    }
    let task = CutoverTask::read(system)?;
    if !task.selection().first().binding_matches(engine)? {
        engine.backup_preparation = Some(task.selection().clone().idle(
            invalid(
                "cutover source changed; original topology retained without opening its targets",
            ),
            BackupPreparationPhase::SourceChanged,
        ));
        return Ok(true);
    }
    let result = (|| {
        let (work, report) = prepare(engine, &task)?;
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
            .selection()
            .clone()
            .idle(error, BackupPreparationPhase::Blocked),
    });
    Ok(true)
}

fn require_capacity(
    root: &DiskRoot,
    task: &CutoverTask,
    report: &BackupCutoverPreparationReport,
    index: usize,
) -> Result<()> {
    let files = [
        (INTENT, task.bytes()?),
        (ACCEPTANCE, acceptance(task, report, index)?),
        (COMPLETE, bounded(report)?),
    ];
    let mut missing = 0u64;
    for (name, bytes) in files {
        let path = root.path().join(name);
        if preparation_path_exists(root, &path)? {
            if root.read_bytes(&path, MAX_TASK)? != bytes {
                return Err(Error::new(
                    ErrorCode::ObjectCollision,
                    "retained cutover input bytes conflict; no record is replaced",
                ));
            }
        } else {
            missing = missing.checked_add(bytes.len() as u64).ok_or_else(|| {
                Error::new(ErrorCode::ResourceLimit, "cutover reservation overflows")
            })?;
        }
    }
    let required = missing
        .checked_add(RESERVE)
        .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "cutover reservation overflows"))?;
    if root.available()? < required {
        return Err(Error::new(
            ErrorCode::NoSpace,
            "one target lacks capacity for its complete cutover inputs",
        ));
    }
    Ok(())
}

fn run(
    task: &CutoverTask,
    work: &[Target],
    initial: &BackupCutoverPreparationReport,
    progress: &Progress,
) -> Result<()> {
    let stop = progress.stop.as_ref();
    let intent = task.bytes()?;
    for (index, (target, job)) in work.iter().zip(task.selection().jobs()).enumerate() {
        preparation_checkpoint(stop)?;
        named_target(&target.root, job)?;
        require_capacity(&target.root, task, initial, index)?;
    }
    for (target, job) in work.iter().zip(task.selection().jobs()) {
        preparation_checkpoint(stop)?;
        named_target(&target.root, job)?;
        install_preparation_bytes(&target.root, &target.root.path().join(INTENT), &intent)?;
    }
    progress.update(|status| {
        status.phase = BackupPreparationPhase::Verifying;
        status.detail = "independently checking every selected subtree member before topology input acknowledgement; old authority unchanged".into();
    })?;
    // Re-read the complete material after intents were persisted. A target's
    // acknowledgement is not inferred from another healthy copy or prior cache.
    let roots = work.iter().map(|target| &target.root).collect::<Vec<_>>();
    let scoped = super::super::verify_at(&task.scoped, &roots, stop)?;
    let observed = report(task, scoped, &roots, stop)?;
    let completed = bounded(&observed)?;
    if completed != bounded(initial)? {
        return Err(invalid("cutover input closure changed during preparation"));
    }
    for (index, (target, job)) in work.iter().zip(task.selection().jobs()).enumerate() {
        preparation_checkpoint(stop)?;
        named_target(&target.root, job)?;
        install_preparation_bytes(
            &target.root,
            &target.root.path().join(ACCEPTANCE),
            &acceptance(task, &observed, index)?,
        )?;
    }
    for (index, (target, job)) in work.iter().zip(task.selection().jobs()).enumerate() {
        preparation_checkpoint(stop)?;
        if target
            .root
            .read_bytes(&target.root.path().join(ACCEPTANCE), MAX_TASK)?
            != acceptance(task, &observed, index)?
        {
            return Err(invalid(
                "a selected subtree member lacks its exact durable input acknowledgement",
            ));
        }
        named_target(&target.root, job)?;
    }
    for (target, job) in work.iter().zip(task.selection().jobs()) {
        preparation_checkpoint(stop)?;
        named_target(&target.root, job)?;
        install_preparation_bytes(&target.root, &target.root.path().join(COMPLETE), &completed)?;
    }
    for (target, job) in work.iter().zip(task.selection().jobs()) {
        named_target(&target.root, job)?;
    }
    progress.update(|status| {
        status.phase = BackupPreparationPhase::CutoverInputsPrepared;
        status.detail = format!(
            "{} explicit whole-unit topology targets acknowledged; full historical DB scopes, active installation, source fence and common authority switch are not implemented",
            work.len()
        );
    })
}

fn spawn(
    engine: &Engine,
    task: CutoverTask,
    system: DiskRoot,
    work: Vec<Target>,
    report: BackupCutoverPreparationReport,
) -> Result<Worker> {
    let locks = engine.retain_writer_locks()?;
    let mut initial = task.selection().status();
    initial.detail =
        "preparing a complete explicit target subtree; this does not register its disks".into();
    let frozen = task.clone();
    task.scoped.objects.native.selection.worker(initial).spawn(
        locks,
        system,
        "zxs-backup-cutover-inputs",
        move |progress| {
            progress.finish(run(&frozen, &work, &report, progress));
        },
    )
}
