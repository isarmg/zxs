//! Explicit native choice, retained as one background operation. Target image
//! completion alone never starts this irreversible decision stage.
use super::*;
use crate::engine::backup_preparation::native_control::databases::runtime::update;

impl Engine {
    /// Select one exact registry/whole-unit placement through ALL original native
    /// member decision slots. Once enough slots select this choice, certified
    /// source cancellation may become impossible. This API does NOT activate a
    /// target writer, publish business success or release the original stop.
    /// The same background task now reserves and finalizes its exact first
    /// native candidate on every original metadata member, never on new voters.
    /// It then verifies every actual routed payload and writes the first native
    /// Publication/seal in that reserved generation. The same original task now
    /// preserves displaced disk databases while installing the published pair
    /// at all original ordinary root names. Old-State catalog access is fenced.
    /// The same task closes old SYSTEM SQLite workers and retains the complete
    /// old main/sidecar set before installing the published ordinary index.
    /// It then installs exact native State/registry/Publication objects at all
    /// ordinary disk addresses and replays target Current from those bytes.
    /// Finally it acquires and durably binds every ordinary target writer lock;
    /// the retained task owns the directory and writer descriptors even after
    /// background completion or failure. These are fences, not DiskWriter I/O.
    /// Query admission and serving activation remain separate: the original
    /// pool stays fenced even after this stage succeeds.
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
    target_writers::resume_existing(&targets)?;
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
    let retained = engine
        .backup_preparation
        .as_ref()
        .map(|worker| {
            worker
                .targets
                .iter()
                .map(Target::retain)
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
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
        Err(error) => {
            let mut worker = task
                .selection()
                .clone()
                .idle(error, BackupPreparationPhase::Blocked);
            worker.targets = retained;
            worker
        }
    });
    Ok(true)
}
struct SystemResources {
    sources: common_databases::Sources,
    index: crate::engine::index::backup_install::QuiescentIndex,
}

fn run(
    task: Task,
    system: &DiskRoot,
    targets: Vec<Target>,
    choices: BackupChoiceContext,
    resources: SystemResources,
    progress: &Mutex<BackupPreparationStatus>,
    stop: &AtomicBool,
) -> Result<()> {
    let SystemResources { sources, index } = resources;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::Verifying;
        status.detail = "verifying every installed whole-unit image before one native registry/placement choice; original writes remain fenced".into();
    })?;
    let prepared = PreparedBackupRegistry::capture(task, targets, stop)?;
    let hash = match choices.recover_decision_if_present(&prepared, stop)? {
        Some(hash) => hash,
        None => {
            serving::require_capacity_before_choice(&prepared, stop)?;
            sources.require_capacity_before_choice(system)?;
            choices.decide(&prepared, stop)?
        }
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
    })?;
    let authority = generation_authority::prepare(&prepared, &decision, stop)?;
    choices.observe_decision(&prepared, stop)?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationAuthorityPrepared;
        status.detail = format!(
            "decision {hash} retains strict native Backup authority {} on {} isolated targets; first publication, active Current/Config, target writer and source release remain unavailable",
            authority.authority_hash, authority.targets,
        );
    })?;
    let first = first_candidate::prepare(&prepared, &decision, stop)?;
    choices.observe_decision(&prepared, stop)?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationFirstCandidatePrepared;
        status.detail = format!(
            "decision {hash} retains exact native first candidate {} and complete snapshot {} on {} isolated targets; no ACCEPT/FINALIZE/Publication, active target writer or source fence release",
            first.candidate_hash, first.snapshot_hash, first.targets,
        );
    })?;
    let (_, candidate) = first_candidate::read_verified(&prepared, &decision, stop)?;
    let finalized = choices.finalize_first_candidate(&prepared, &candidate, stop)?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationFirstCandidateFinalized;
        status.detail = format!(
            "decision {hash} has actual first candidate {} ACCEPT/FINALIZE {} on all {} original members; no Publication, target writer or source fence release",
            finalized.candidate_hash,
            finalized.finalize_hash,
            finalized.original_metadata_members.len(),
        );
    })?;
    let publication = choices.publish_first_candidate(&prepared, &candidate, stop)?;
    let published = publication.report();
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationFirstCandidatePublished;
        status.detail = format!(
            "decision {hash} has actual reserved-generation Publication {} with {} verified payload receipts and all {} original member acknowledgements; active Config/Current, target writer and source fence release remain unavailable",
            published.publication_hash,
            published.payload_receipts,
            published.original_metadata_members.len(),
        );
    })?;
    let retained = first_publication::prepare(&prepared, &decision, &candidate, publication, stop)?;
    choices.observe_first_publication(&prepared, &candidate, stop)?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationFirstPublicationRetained;
        status.detail = format!(
            "decision {hash} retains actual Publication {} and complete payload proofs on all {} selected targets, including empty histories; detached inspection requires independently retained decision and publication hashes; no active Config/Current, target writer or source release",
            retained.publication.publication_hash, retained.targets,
        );
    })?;
    let expected =
        crate::BackupFirstPublicationHash::parse(&retained.publication.publication_hash)?;
    let configured = configuration::inspect(&prepared, &decision, &expected, stop)?;
    choices.observe_first_publication(&prepared, &candidate, stop)?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationConfigurationResolved;
        status.detail = format!(
            "decision {hash} resolves complete fanout Config {} and exact native registry {} from actual Publication {}; all original voters/data remain, target I/O/Current/writer and source fence release are not installed",
            configured.config_hash,
            configured.registry.root_hash(),
            configured.publication.publication.publication_hash,
        );
    })?;
    let installed = published_registry::install(&prepared, &decision, &expected, stop)?;
    choices.observe_first_publication(&prepared, &candidate, stop)?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationRegistryInstalled;
        status.detail = format!(
            "decision {hash} retains exact native registry {} and Config on all {} original targets, with {} branch and {} directory scopes; frozen read view only, ordinary Current/target writer and source fence release remain unavailable",
            installed.configuration.registry.root_hash(),
            installed.configuration.publication.targets,
            installed.branch_scopes,
            installed.directory_scopes,
        );
    })?;
    let serving = serving::install(&prepared, &decision, &expected, stop)?;
    choices.observe_first_publication(&prepared, &candidate, stop)?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationServingImagesInstalled;
        status.detail = format!(
            "decision {hash} independently installed exact published entity/SQLite/locator images on all {} targets after Publication {}; frozen namespace-bound body reads only, ordinary Current/target writer, final SYSTEM/disk installation and source fence release remain unavailable",
            serving.targets.len(),
            serving
                .registry
                .configuration
                .publication
                .publication
                .publication_hash,
        );
    })?;
    let scopes = published_scopes::install(&prepared, &decision, &expected, stop)?;
    choices.observe_first_publication(&prepared, &candidate, stop)?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationPublishedScopesInstalled;
        status.detail = format!(
            "decision {hash} retains published SYSTEM/current-history scopes and each target's own root databases on all {} targets; old histories remain bound to their own disks, source SYSTEM and original disks are unchanged; ordinary Current/writer and source fence release remain unavailable",
            scopes.targets.len(),
        );
    })?;
    let common = common_databases::install(
        &common_databases::Context {
            system,
            sources: &sources,
            choices: &choices,
        },
        &prepared,
        &decision,
        &expected,
        stop,
    )?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationCommonDatabasesInstalled;
        status.detail = format!(
            "decision {hash} retains common published database set {} at SYSTEM, all {} original disks and every target; immutable generation-local copies only, old active database aliases/Current/writer and source fence remain unchanged",
            common.common_manifest_hash,
            common.original_disks.len(),
        );
    })?;
    let current = current_read::observe(
        &common_databases::Context {
            system,
            sources: &sources,
            choices: &choices,
        },
        &prepared,
        &decision,
        &expected,
        stop,
    )?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationCurrentReadAdmitted;
        status.detail = format!(
            "decision {hash} admits read-only Current namespaces on all {} targets against actual Publication and common database set {}; {} readable branch scopes and {} directory scopes, no active Engine Current/writer or source fence release",
            current.targets,
            current.common.common_manifest_hash,
            current.readable_branch_scopes,
            current.directory_scopes,
        );
    })?;
    let aliases = database_aliases::install(
        &common_databases::Context {
            system,
            sources: &sources,
            choices: &choices,
        },
        &prepared,
        &decision,
        &expected,
        stop,
    )?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationDatabaseAliasesInstalled;
        status.detail = format!(
            "decision {hash} resolves {} exact published database aliases in set {}; generation-local reads only, no ordinary active alias/writer switch or source fence release",
            aliases.aliases.len(),
            aliases.aliases_hash,
        );
    })?;
    let roots = root_databases::install(
        &common_databases::Context {
            system,
            sources: &sources,
            choices: &choices,
        },
        &prepared,
        &decision,
        &expected,
        stop,
    )?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationOrdinaryRootDatabasesInstalled;
        status.detail = format!(
            "decision {hash} installed published current/history pairs at all {} original ordinary disk roots and observed all {} target root pairs in set {}; displaced old files are retained, old catalog access is fenced; SYSTEM alias, active Current/writer and source release remain unavailable",
            roots.original_disks.len(),
            roots.selected_targets,
            roots.installation_hash,
        );
    })?;
    let installed_system = system_database::install(
        &common_databases::Context {
            system,
            sources: &sources,
            choices: &choices,
        },
        &prepared,
        &decision,
        &expected,
        &index,
        stop,
    )?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationOrdinarySystemDatabaseInstalled;
        status.detail = format!(
            "decision {hash} installed exact ordinary SYSTEM database {} under selection {}, retaining all {} old main/sidecar files after SQLite quiescence; ordinary query admission, active Current/writer, next publication and source release remain unavailable",
            installed_system.system_database_hash,
            installed_system.installation_hash,
            installed_system.retained_old_files,
        );
    })?;
    let native = native_metadata::install(
        &common_databases::Context {
            system,
            sources: &sources,
            choices: &choices,
        },
        &prepared,
        &decision,
        &expected,
        stop,
    )?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationOrdinaryNativeMetadataInstalled;
        status.detail = format!(
            "decision {hash} installed exact ordinary native set {} with {} objects/{} bytes per disk on all {} original disks and {} selected targets; target Current replay reads actual ordinary objects, but active Config/generation locator, writer, next publication and source release remain unavailable",
            native.installation_hash,
            native.objects_per_disk,
            native.bytes_per_disk,
            native.original_disks,
            native.selected_targets,
        );
    })?;
    let writers = target_writers::install(
        &common_databases::Context {
            system,
            sources: &sources,
            choices: &choices,
        },
        &prepared,
        &decision,
        &expected,
        stop,
    )?;
    update(system, progress, |status| {
        status.phase = BackupPreparationPhase::GenerationTargetWriterFencesInstalled;
        status.detail = format!(
            "decision {hash} durably bound all {} actual target writer files in set {}; this Engine task retains directory/writer ownership after completion; active selector, disk.json, Current, writer I/O, next publication and source release remain unavailable",
            writers.targets.len(),
            writers.fencing_hash,
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
    let sources = common_databases::Sources::capture(engine, &system)?;
    let index = engine.quiesce_backup_system_index()?;
    let resources = SystemResources { sources, index };
    let mut initial = task.selection().status();
    initial.phase = BackupPreparationPhase::Verifying;
    initial.detail = "recovering only the original native registry/placement task; no resampling or source release".into();
    let state = Arc::new(Mutex::new(initial));
    let stop = Arc::new(AtomicBool::new(false));
    let progress = Arc::clone(&state);
    let cancel = Arc::clone(&stop);
    let frozen = task.clone();
    let retained_targets = targets
        .iter()
        .map(Target::retain)
        .collect::<Result<Vec<_>>>()?;
    let thread = std::thread::Builder::new()
        .name("zxs-backup-registry-decision".into())
        .spawn(move || {
            let _locks = locks;
            if let Err(error) = run(
                frozen, &system, targets, choices, resources, &progress, &cancel,
            ) && let Err(persistence) = update(&system, &progress, |status| {
                status.phase = BackupPreparationPhase::Blocked;
                status.error_code = Some(error.code());
                status.detail = error.to_string();
            }) {
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
        targets: retained_targets,
    })
}
