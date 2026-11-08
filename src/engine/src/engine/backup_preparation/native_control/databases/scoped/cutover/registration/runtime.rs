//! Explicit native choice, retained as one background operation. Target image
//! completion alone never starts this irreversible decision stage.
use super::*;
use crate::engine::backup_preparation::native_control::databases::runtime::update;

impl Engine {
    /// Select one exact registry/whole-unit placement through ALL original native
    /// member decision slots. Once enough slots select this choice, certified
    /// source cancellation may become impossible. This API does NOT activate a
    /// target writer, publish business success or release the original stop.
    /// Hosts must authenticate this separate, explicit request. Ordinary settings
    /// and helper admission do not call it automatically.
    pub fn start_backup_registry_placement_decision(
        &mut self,
        operation: OperationId,
        principal: &str,
    ) -> Result<BackupPreparationStatus> {
        Self::require_actor(principal)?;
        self.require_no_commit_preparation()?;
        let task = Task {
            format: FORMAT.into(),
            installation: InstallationTask::current(self, operation, principal)?,
        };
        if self
            .backup_preparation
            .as_ref()
            .and_then(|worker| worker.thread.as_ref())
            .is_some_and(|thread| !thread.is_finished())
        {
            return Err(Error::new(
                ErrorCode::WorkspaceBusy,
                "wait for the retained worker before selecting registry and placement",
            ));
        }
        if let Some(worker) = self.backup_preparation.as_mut() {
            worker.join()?;
        }
        let system = DiskRoot::open(&self.root)?;
        if task.installation.bytes()? != InstallationTask::read(&system)?.bytes()? {
            return Err(invalid(
                "registry decision needs the same persisted entity task",
            ));
        }
        if preparation_path_exists(&system, &system.path().join(TASK))? {
            if Task::read(&system)?.bytes()? != task.bytes()? {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "registry decision task belongs to another original fence",
                ));
            }
            resume(self, &system)?;
        } else {
            let (targets, choices) = acquire(self, &task)?;
            install_preparation_bytes(&system, &system.path().join(TASK), &task.bytes()?)?;
            self.backup_preparation =
                Some(match spawn(self, task.clone(), system, targets, choices) {
                    Ok(worker) => worker,
                    Err(error) => task
                        .selection()
                        .clone()
                        .idle(error, BackupPreparationPhase::Blocked),
                });
        }
        self.backup_preparation_status()
            .ok_or_else(|| invalid("registry task has no diagnostic"))
    }
}
pub(super) fn acquire(engine: &Engine, task: &Task) -> Result<(Vec<Target>, BackupChoiceContext)> {
    // The native capability and all source seals are admitted before a target
    // name is reopened; SYSTEM JSON is only the owner intent, not the authority.
    task.installation.require_current(engine)?;
    let request = &task.installation.evidence.task.request;
    let choices = engine.capture_backup_choice_context(request.operation_id, &request.principal)?;
    let targets = installation::acquire(engine, &task.installation)?;
    Ok((targets, choices))
}
pub(in crate::engine::backup_preparation) fn resume(
    engine: &mut Engine,
    system: &DiskRoot,
) -> Result<bool> {
    if !preparation_path_exists(system, &system.path().join(TASK))? {
        return Ok(false);
    }
    let task = Task::read(system)?;
    let result = (|| {
        let (targets, choices) = acquire(engine, &task)?;
        spawn(
            engine,
            task.clone(),
            DiskRoot::open(system.path())?,
            targets,
            choices,
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
fn run(
    task: Task,
    system: &DiskRoot,
    targets: Vec<Target>,
    choices: BackupChoiceContext,
    progress: &Mutex<BackupPreparationStatus>,
    stop: &AtomicBool,
) -> Result<()> {
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::Verifying;
        status.detail = "verifying every installed whole-unit image before one native registry/placement choice; original writes remain fenced".into();
    })?;
    let prepared = PreparedBackupRegistry::capture(task, targets, stop)?;
    let hash = match choices.recover_decision_if_present(&prepared, stop)? {
        Some(hash) => hash,
        None => choices.decide(&prepared, stop)?,
    };
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::RegistryPlacementDecided;
        status.completed_files = status.total_files;
        status.completed_bytes = status.total_bytes;
        status.detail = format!(
            "all original members selected registry/placement decision {hash}; every target retains the same certificate; final active route/writer is absent and source remains stopped"
        );
    })?;
    // Never derive a writer route from the planned receipt vector or cached
    // progress. This read gate observes every original real decision slot.
    let decision = choices.observe_decision(&prepared, stop)?;
    let routes = writer_routes::prepare(&prepared, &decision, stop)?;
    // Source locks are retained throughout, but exact native source evidence is
    // still checked before the observable preparation result advances.
    choices.observe_decision(&prepared, stop)?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::WriterRoutesPrepared;
        status.detail = format!(
            "decision {hash} retains complete whole-unit fanout material {} on all {} selected targets; no generation publication or writer activation exists, source remains stopped",
            routes.material_hash,
            routes.targets.len(),
        );
    })?;
    let inputs = generation_inputs::prepare(&prepared, &decision, stop)?;
    choices.observe_decision(&prepared, stop)?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationInputsPrepared;
        status.detail = format!(
            "decision {hash} retains unpublished native routing snapshot {} on all {} targets; final Backup authority, publication and active writers are absent; source remains stopped",
            inputs.routing_snapshot_hash, inputs.targets,
        );
    })?;
    let control = generation_control::prepare(&prepared, &decision, &choices, stop)?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationControlPrepared;
        status.detail = format!(
            "decision {hash} retains normal native control archive {} including all original fenced journals on {} targets; no successor publication or target writer exists; source remains stopped",
            control.native_control_root, control.targets,
        );
    })
}
fn spawn(
    engine: &Engine,
    task: Task,
    system: DiskRoot,
    targets: Vec<Target>,
    choices: BackupChoiceContext,
) -> Result<Worker> {
    let locks = engine.retain_writer_locks()?;
    let mut initial = task.selection().status();
    initial.phase = BackupPreparationPhase::Verifying;
    initial.detail = "recovering only the original native registry/placement task; no resampling or source release".into();
    let state = Arc::new(Mutex::new(initial));
    let stop = Arc::new(AtomicBool::new(false));
    let progress = Arc::clone(&state);
    let cancel = Arc::clone(&stop);
    let frozen = task.clone();
    let thread = std::thread::Builder::new()
        .name("zxs-backup-registry-decision".into())
        .spawn(move || {
            let _locks = locks;
            if let Err(error) = run(frozen, &system, targets, choices, &progress, &cancel)
                && let Err(persistence) = update(&system, &progress, |status| {
                    status.phase = BackupPreparationPhase::Blocked;
                    status.error_code = Some(error.code());
                    status.detail = error.to_string();
                })
            {
                progress
                    .lock()
                    .expect("invariant: registry diagnostic mutex poisoned")
                    .detail = format!("{error}; progress persistence failed: {persistence}");
            }
        })?;
    let job = task.selection().first().clone();
    let batch = match task
        .installation
        .evidence
        .task
        .scoped
        .objects
        .native
        .selection
    {
        Selection::Single(_) => None,
        Selection::Batch(batch) => Some(batch),
    };
    Ok(Worker {
        job,
        batch,
        state,
        stop,
        thread: Some(thread),
    })
}
