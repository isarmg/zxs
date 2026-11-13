//! The one final Backup switch. No progress/report grants activation. Native
//! first Publication, independent ordinary stores and all actual writer owners
//! are retained until selectors, Current and SYSTEM cross the same barrier.
use super::*;
use crate::engine::backup_preparation::ServingBarrier;
use crate::engine::disk_registry::current::backup::BackupNamespaceReplay;
use crate::engine::membership_transition::{
    AdmittedBackupDecision, BackupFirstPublicationHash, PreparedBackupFirstPublication,
    pending::activation::BackupWriterIoConfig,
};
use crate::history_package::AdmittedState;
use crate::journal::OpenJournal;

mod original_objects;
mod recovery;
mod roots;

const INTENT: &str = "backup-preparation/serving-switch.json";
const SELECTED: &str = "backup-serving-selected.json";
const COMPLETE: &str = "backup-serving-installed.json";
const MAX_PLAN: usize = 256 * 1024;

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Selection {
    format: String,
    operation: OperationId,
    authority: String,
    source_fence: String,
    decision: String,
    candidate: String,
    publication: String,
    config_hash: String,
    system_database_hash: String,
    registry: DiskRegistryCommitment,
    ordinary_set: String,
    system: PhysicalBinding,
    system_writer: PhysicalBinding,
    original_objects: Vec<original_objects::Recipe>,
    original_roots: Vec<roots::Recipe>,
    target_writers: Vec<TargetBinding>,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct TargetBinding {
    identity: DiskRegistrationIdentity,
    writer_device: u64,
    writer_inode: u64,
    ordinary_recipe: String,
    ordinary_entities: String,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Completion {
    format: String,
    selection_hash: String,
    original_objects: Vec<String>,
    native_files: Vec<crate::disk::FileStamp>,
}

struct Native {
    registry: published_registry::Plan,
    publication: PreparedBackupFirstPublication,
    graph: crate::VerifiedDiskRegistry,
    published: AdmittedState,
    replay: BackupNamespaceReplay,
    writer: BackupWriterIoConfig,
    ordinary: BackupOrdinaryServingReport,
}
/// Process-local ownership of this exact final handoff. It is not serialized,
/// does not release the source stop, and is never constructed from progress.
/// Every retry rereads the common native proof and exact selected file set.
pub(in crate::engine) struct ServingSwitch {
    registry: published_registry::Plan,
    publication: PreparedBackupFirstPublication,
    published: AdmittedState,
    writer: BackupWriterIoConfig,
    current: Arc<crate::engine::disk_registry::current::CurrentNamespaces>,
    barrier: ServingBarrier,
    roots: roots::Roots,
    targets: Vec<Arc<DiskHandle>>,
    writers: Vec<(String, DiskWriter)>,
    selection: Vec<u8>,
    completed_objects: Vec<String>,
    target_count: usize,
}

impl Native {
    fn capture(
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let ordinary = ordinary_serving::observe(context, prepared, decision, expected, stop)?;
        let registry = published_registry::Plan::capture(prepared, decision, expected, stop)?;
        let candidate = registry.candidate();
        let publication = context
            .choices
            .read_first_publication(prepared, candidate, stop)?;
        let target = &prepared.targets[0];
        let graph = registry.verify(target, stop)?;
        // The full installed ordinary metadata is the source, not a report,
        // SYSTEM SQL projection or a peer substituted after a read failure.
        let published = candidate.read_state_at(
            &target
                .root
                .private_child(&target.root.path().join("metadata"))?,
            stop,
        )?;
        let writer = candidate
            .writer_configuration(
                &publication,
                generation_control::retained_archive(prepared, 0)?,
                stop,
            )?
            .bind_published_continuation(candidate, &publication, graph.clone(), &published)?;
        let replay = published_registry::archive_replay(prepared, 0, candidate, stop)?;
        Ok(Self {
            registry,
            publication,
            graph,
            published,
            replay,
            writer,
            ordinary,
        })
    }
    fn barrier(&self) -> ServingBarrier {
        let first = self.registry.candidate();
        ServingBarrier {
            operation: first.candidate().operation_id,
            authority: first.authority_hash().into(),
            candidate: first.hash().into(),
            publication: self.publication.report().publication_hash.clone(),
            registry: self.graph.commitment().clone(),
            config_hash: self.registry.configuration().config_hash.clone(),
            first_sequence: first.sequence(),
            first_snapshot: first.snapshot_hash().into(),
            system_database_hash: self
                .ordinary
                .original
                .targets
                .seals
                .io
                .writers
                .native
                .system
                .system_database_hash
                .clone(),
            replayed_current: false,
        }
    }
}

fn require_bytes(root: &DiskRoot, name: &str, expected: &[u8], maximum: usize) -> Result<()> {
    if root.read_bytes(&root.path().join(name), maximum)? != expected {
        return Err(invalid(
            "Backup serving selection or completion differs from the original common native switch",
        ));
    }
    Ok(())
}
fn bounded(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > MAX_PLAN {
        return Err(invalid("Backup serving switch descriptor exceeds 256 KiB"));
    }
    Ok(bytes)
}

impl Engine {
    /// Complete the SAME already-published Backup operation as one ordinary
    /// pool. The background copier must have stopped; all original and selected
    /// roots are mandatory. This does not select a new decision or erase any
    /// displaced old object. Failure remains fenced and needs the same native
    /// operation's recovery, never a new copy task or a rollback selector.
    pub fn complete_backup_registry_placement_decision(
        &mut self,
        operation: OperationId,
        principal: &str,
    ) -> Result<OperationResult> {
        Self::require_actor(principal)?;
        self.require_no_commit_preparation()?;
        if let Some(admitted) = &self.backup_serving {
            if admitted.barrier.operation != operation
                || self
                    .state
                    .operation_principals
                    .get(&operation)
                    .map(String::as_str)
                    != Some(principal)
            {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "completed Backup retry differs from its original operation or actor",
                ));
            }
            return self.complete_backup_serving_open();
        }
        if self
            .backup_preparation
            .as_ref()
            .and_then(|worker| worker.thread.as_ref())
            .is_some_and(|thread| !thread.is_finished())
        {
            return Err(Error::new(
                ErrorCode::WorkspaceBusy,
                "wait for the original Backup worker before its final common switch",
            ));
        }
        if let Some(worker) = self.backup_preparation.as_mut() {
            worker.join()?;
        }
        let system = DiskRoot::open(&self.root)?;
        let task = Task::read(&system)?;
        if task.selection().first().request.operation_id != operation
            || task.selection().first().request.principal != principal
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "Backup final switch does not own the original operation and principal",
            ));
        }
        if self.backup_serving_switch.is_none() {
            task.installation.require_current(self)?;
        }
        self.blocked = true;
        let result = (|| {
            if self.backup_serving_switch.is_none() {
                prepare_switch(self, task, &system, &AtomicBool::new(false))?;
            }
            self.finish_backup_serving_switch(operation, principal)
        })();
        if let Err(error) = &result
            && let Some(worker) = &self.backup_preparation
        {
            let mut failed = worker.status();
            failed.phase = BackupPreparationPhase::Blocked;
            failed.error_code = Some(error.code());
            failed.detail = format!(
                "Backup final switch remains fenced; explicitly resume this same operation with its retained owners. If the process was lost, use same-operation cold recovery: {error}"
            );
            // This is the diagnostic cache only. Native selections and held
            // owners remain untouched, including on a poisoned cache.
            match worker.state.lock() {
                Ok(mut status) => *status = failed,
                Err(poisoned) => *poisoned.into_inner() = failed,
            }
        }
        result
    }
}

fn prepare_switch(
    engine: &mut Engine,
    task: Task,
    system: &DiskRoot,
    stop: &AtomicBool,
) -> Result<()> {
    let (targets, choices) = runtime::acquire(engine, &task)?;
    let sources = common_databases::Sources::capture(engine, system)?;
    let prepared = PreparedBackupRegistry::capture(task, targets, stop)?;
    let decision = choices.observe_decision(&prepared, stop)?;
    let (_, first) = first_candidate::read_verified(&prepared, &decision, stop)?;
    let expected = BackupFirstPublicationHash::parse(
        &choices
            .read_first_publication(&prepared, &first, stop)?
            .report()
            .publication_hash,
    )?;
    drop(first);
    let context = common_databases::Context {
        system,
        sources: &sources,
        choices: &choices,
    };
    let native = Native::capture(&context, &prepared, &decision, &expected, stop)?;
    let objects = original_objects::Objects::capture(&prepared, &native.registry, &sources, stop)?;
    let io_hash = &native.ordinary.original.targets.seals.io.admission_hash;
    let mut target_handles = Vec::with_capacity(prepared.targets.len());
    let mut writers = Vec::with_capacity(prepared.targets.len());
    for target in &prepared.targets {
        let (handle, writer) = target.serving_resources(io_hash)?;
        writers.push((handle.seal.disk_id.clone(), writer));
        target_handles.push(handle);
    }
    let roots = roots::Roots::capture(engine, &native, target_handles.clone())?;
    let selection = bounded(&Selection {
        format: "zxs-backup-serving-switch".into(),
        operation: native.registry.candidate().candidate().operation_id,
        authority: native.registry.candidate().authority_hash().into(),
        source_fence: prepared.source_fence_hash().into(),
        decision: decision.hash().as_str().into(),
        candidate: native.registry.candidate().hash().into(),
        publication: expected.as_str().into(),
        registry: native.graph.commitment().clone(),
        config_hash: native.registry.configuration().config_hash.clone(),
        system_database_hash: native
            .ordinary
            .original
            .targets
            .seals
            .io
            .writers
            .native
            .system
            .system_database_hash
            .clone(),
        ordinary_set: native.ordinary.installation_hash.clone(),
        system: sources.system_binding().clone(),
        system_writer: sources.system_writer_binding().clone(),
        original_objects: objects.recipes.clone(),
        original_roots: roots.recipes(),
        target_writers: native
            .ordinary
            .original
            .targets
            .seals
            .io
            .writers
            .targets
            .iter()
            .zip(&native.ordinary.targets)
            .map(|(writer, ordinary)| {
                assert_eq!(
                    writer.identity, ordinary.identity,
                    "invariant: ordinary target report follows the admitted writer plan"
                );
                TargetBinding {
                    identity: writer.identity.clone(),
                    writer_device: writer.writer_device,
                    writer_inode: writer.writer_inode,
                    ordinary_recipe: ordinary.recipe_hash.clone(),
                    ordinary_entities: ordinary.entities_hash.clone(),
                }
            })
            .collect(),
    })?;
    if preparation_path_exists(system, &system.path().join(COMPLETE))? {
        return Err(invalid(
            "completed Backup switch must use native completed-generation recovery, not preparation",
        ));
    }
    let pool = &prepared.task.selection().first().pool_id;
    objects.require_capacity(
        &native.registry,
        &sources,
        pool,
        sources.metadata_reserve_bytes(),
        stop,
    )?;
    roots.visit(|root| {
        if root.available()? < RESERVE + 2 * selection.len() as u64 {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "Backup switch lacks common selection/completion space",
            ));
        }
        Ok(())
    })?;
    // Record the whole bounded set before one actual original object changes.
    install_preparation_bytes(system, &system.path().join(INTENT), &selection)?;
    install_preparation_bytes(system, &system.path().join(SELECTED), &selection)?;
    roots.visit(|root| install_preparation_bytes(root, &root.path().join(SELECTED), &selection))?;
    require_bytes(system, INTENT, &selection, MAX_PLAN)?;
    roots.visit(|root| require_bytes(root, SELECTED, &selection, MAX_PLAN))?;
    let completed_objects = objects.install(&native.registry, &sources, pool, &selection, stop)?;
    choices.observe_first_publication(&prepared, native.registry.candidate(), stop)?;
    sources.for_each_disk(
        stop,
        |root, identity| {
            let disk = engine
                .config
                .disks
                .iter()
                .find(|disk| disk.disk_id == identity.disk_id)
                .expect("invariant: original Sources retain exactly the fenced Engine disks");
            native.publication.verify_initial_ordinary_payloads(
                root,
                disk,
                native.registry.candidate(),
                stop,
            )
        },
        pool,
    )?;
    sources.recheck(system)?;
    // Keep the full native continuation and the actual old/new seal handles
    // BEFORE changing the first selector. A retry cannot recapture old seals
    // after a partial exchange, nor should it release these owners to reopen.
    let barrier = native.barrier();
    let current = Arc::new(native.replay.finish_serving(
        &native.publication,
        native.registry.candidate(),
        &native.graph,
        &native.published,
        &barrier,
    )?);
    engine.backup_serving_switch = Some(ServingSwitch {
        registry: native.registry,
        publication: native.publication,
        published: native.published,
        writer: native.writer,
        current,
        barrier,
        roots,
        targets: target_handles,
        writers,
        selection,
        completed_objects,
        target_count: native.ordinary.targets.len(),
    });
    Ok(())
}

impl Engine {
    fn finish_backup_serving_switch(
        &mut self,
        operation: OperationId,
        principal: &str,
    ) -> Result<OperationResult> {
        let pending = self
            .backup_serving_switch
            .take()
            .ok_or_else(|| invalid("Backup final switch has no retained original continuation"))?;
        // All fallible handoff work is performed while the original Engine
        // stays fenced. Failure puts back the SAME proof, roots and owners.
        let prepared = (|| {
            let first = pending.registry.candidate();
            if first.result().operation_id != operation
                || pending
                    .published
                    .state()
                    .operation_principals
                    .get(&operation)
                    .map(String::as_str)
                    != Some(principal)
                || !self.blocked
                || self.pending_membership.is_some()
                || self.pending_disk_exit.is_some()
                || self.history_move_choice.is_some()
                || self.history_move_prepare.is_some()
            {
                return Err(invalid(
                    "Backup live continuation is not its exact stopped original operation",
                ));
            }
            self.require_no_commit_preparation()?;
            #[cfg(feature = "fault-injection")]
            self.failpoint("after_backup_serving_selected")?;
            let system = DiskRoot::open(&self.root)?;
            let mut owners = self._locks.retain()?;
            owners.extend(pending.writers.to_vec());
            for handle in pending.roots.handles() {
                let disk = pending
                    .writer
                    .config()
                    .disks
                    .iter()
                    .find(|disk| disk.disk_id == handle.seal.disk_id)
                    .ok_or_else(|| {
                        invalid("Backup retained root is not in its native membership")
                    })?;
                owners.verify_disk(disk, &handle.root)?;
            }
            let _activity = owners.activity()?;
            let bootstrap = crate::engine::membership_transition::pending::activation::backup::startup::PreparedBackupBootstrap::select(
                self,
                &pending.writer,
                first,
                &pending.publication,
                &pending.targets,
            )?;
            let completed = recovery::verify_live_selection(
                &system,
                bootstrap.admission(),
                &pending.selection,
                &pending.completed_objects,
                &AtomicBool::new(false),
            )?;
            pending.roots.apply(&pending.selection, completed)?;
            let completion = bounded(&Completion {
                format: "zxs-backup-serving-completed".into(),
                selection_hash: blake3::hash(&pending.selection).to_hex().to_string(),
                original_objects: pending.completed_objects.clone(),
                native_files: pending.roots.completion()?,
            })?;
            pending.roots.visit(|root| {
                install_preparation_bytes(root, &root.path().join(COMPLETE), &completion)
            })?;
            install_preparation_bytes(&system, &system.path().join(COMPLETE), &completion)?;
            pending
                .roots
                .visit(|root| require_bytes(root, COMPLETE, &completion, MAX_PLAN))?;
            require_bytes(&system, COMPLETE, &completion, MAX_PLAN)?;
            bootstrap.publish(&pending.barrier)?;
            #[cfg(feature = "fault-injection")]
            self.failpoint("after_backup_bootstrap_published")?;
            let config = pending
                .writer
                .config()
                .clone()
                .with_workspace_limits(Some(self.config.effective_workspace_limits()))?
                .with_backup_serving_resources(&pending.barrier)?;
            // Retain all actual target owners before generation binding can
            // fail. No directory or writer is unlocked and reacquired.
            self._locks.extend(pending.writers.to_vec());
            let resources = Arc::new(DiskResources::generation_handles(
                &config,
                pending.roots.handles(),
            )?);
            #[cfg(feature = "fault-injection")]
            self.failpoint("after_backup_serving_resources")?;
            let mut journals = BTreeMap::new();
            for disk in config.metadata_members() {
                let root = &resources.get(disk)?.root;
                pending.publication.original_journal_binding(
                    root,
                    disk,
                    first,
                    &AtomicBool::new(false),
                )?;
                let opened = OpenJournal::scan_configured_at(root, disk, &config)?;
                if opened.scan.torn_tail || opened.scan.records.len() != 3 {
                    return Err(invalid(
                        "live first Backup switch changed its exact original three-frame journal",
                    ));
                }
                let (writer, _) = opened.into_writer()?;
                journals.insert(disk.disk_id.clone(), writer);
            }
            let (_, history_roots) = crate::history_package::bound_history_objects_with_roots_at(
                &resources,
                &config.metadata_members().cloned().collect::<Vec<_>>(),
                first.candidate(),
                Some(&pending.published),
            )?;
            let snapshot = first.snapshot_hash().to_owned();
            let next_slot = first
                .sequence()
                .checked_add(1)
                .ok_or_else(|| invalid("Backup first publication exhausts journal sequence"))?;
            let admitted = crate::engine::backup_preparation::ServingAdmission::capture(
                pending.barrier.clone(),
                Arc::clone(&pending.current),
                first,
                pending.target_count,
            )?;
            Ok::<_, Error>((
                config,
                resources,
                journals,
                history_roots,
                snapshot,
                next_slot,
                admitted,
            ))
        })();
        let (config, resources, journals, history_roots, snapshot, next_slot, admitted) =
            match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    self.backup_serving_switch = Some(pending);
                    return Err(error);
                }
            };
        // No fallible operation may split this in-memory handoff. Later
        // Current/SYSTEM/config failures retry through backup_serving, with the
        // original stop still retained until complete_backup_serving_open.
        self.config = config;
        self.disk_resources = resources;
        self.journals = journals;
        self.named_namespaces.install_state(&mut self.state, Arc::new(pending.published.into_state()));
        self.snapshot = Some(snapshot);
        self.next_slot = next_slot;
        self.backup_serving = Some(admitted);
        self.named_namespaces.install_current(&mut self.current_namespaces, Some(pending.current));
        self.workspace_namespace_baselines.clear();
        self.published_state_base = None;
        self.metadata_integrity.invalidate();
        self.bind_recovered_history_roots(history_roots);
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_backup_current_installed")?;
        self.complete_backup_serving_open()
    }
}
