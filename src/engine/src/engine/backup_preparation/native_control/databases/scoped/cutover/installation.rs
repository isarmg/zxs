//! Recover the same source-fenced operation and install isolated target entities.
//! This does not choose a successor authority, release a fence or serve a disk.
use super::*;
use crate::engine::backup_preparation::native_control::databases::scoped::encoding_memo;
use crate::container_storage::backup_entities::Image;

const FORMAT: &str = "zxs-backup-fenced-entity-installation";
const TASK: &str = "backup-preparation/target-entities.json";
const INTENT: &str = "target-entities-intent.json";
const ACCEPTANCE: &str = "target-entities-installed.json";
const COMPLETE: &str = "target-entity-set-installed.json";
const MAX_INTENT: usize = 16 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Task {
    format: String,
    pub(super) source_fence_hash: String,
    pub(super) evidence: BackupSourceFenceEvidence,
}
impl Task {
    pub(super) fn selection(&self) -> &Selection {
        self.evidence.task.selection()
    }
    pub(super) fn validate(&self) -> Result<()> {
        if self.format != FORMAT
            || self.source_fence_hash.len() != 64
            || !self
                .source_fence_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid("unsupported fenced target installation task"));
        }
        let task = &self.evidence.task;
        let job = task.selection().first();
        self.evidence.validate(
            &crate::ValidatedConfig::try_from(task.original.clone())?,
            task.request.operation_id,
            &task.request.principal,
            job.request.expected_sequence,
            &job.snapshot,
        )
    }
    pub(super) fn bytes(&self) -> Result<Vec<u8>> {
        let bytes = canonical_json(self)?;
        if bytes.len() > MAX_INTENT {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "fenced target installation exceeds intent bound",
            ));
        }
        Ok(bytes)
    }
    pub(super) fn current(
        engine: &Engine,
        operation: OperationId,
        principal: &str,
    ) -> Result<Self> {
        let (source_fence_hash, evidence) = engine.fenced_backup_inputs(operation, principal)?;
        Ok(Self {
            format: FORMAT.into(),
            source_fence_hash,
            evidence,
        })
    }
    pub(super) fn read(system: &DiskRoot) -> Result<Self> {
        let task: Self = crate::journal::decode_proof(
            &system.read_bytes(&system.path().join(TASK), MAX_INTENT)?,
        )?;
        task.validate()?;
        Ok(task)
    }
    /// Read only the original target intent. It conveys no authority; callers
    /// must bind it to the independently trusted native decision before use.
    pub(super) fn read_target(root: &DiskRoot) -> Result<Self> {
        let task: Self =
            crate::journal::decode_proof(&root.read_bytes(&root.path().join(INTENT), MAX_INTENT)?)?;
        task.validate()?;
        Ok(task)
    }
    pub(super) fn require_current(&self, engine: &Engine) -> Result<()> {
        let request = &self.evidence.task.request;
        if self.bytes()?
            != Self::current(engine, request.operation_id, &request.principal)?.bytes()?
        {
            return Err(invalid(
                "target installation differs from the admitted original native fence; targets untouched",
            ));
        }
        Ok(())
    }
}
pub(super) use crate::engine::backup_preparation::target_resources::Target;

impl Engine {
    /// Start/continue the exact source-fenced task's target entity installation.
    /// All original members must already have the same durable native barrier.
    /// Target data is copied from target-local verified packages, not moved from
    /// the source. This remains a whole-pool write stop, NOT migration success.
    /// Hosts must authenticate the caller; no HTTP/helper entry is installed.
    pub fn start_backup_target_installation(
        &mut self,
        operation: OperationId,
        principal: &str,
    ) -> Result<BackupPreparationStatus> {
        Self::require_actor(principal)?;
        self.require_no_commit_preparation()?;
        let task = Task::current(self, operation, principal)?;
        if self
            .backup_preparation
            .as_ref()
            .and_then(|worker| worker.thread.as_ref())
            .is_some_and(|thread| !thread.is_finished())
        {
            return Err(Error::new(
                ErrorCode::WorkspaceBusy,
                "wait for the same retained backup worker; no parallel image installer started",
            ));
        }
        if let Some(worker) = self.backup_preparation.as_mut() {
            worker.join()?;
        }
        let system = DiskRoot::open(&self.root)?;
        if preparation_path_exists(&system, &self.root.join(TASK))? {
            if Task::read(&system)?.bytes()? != task.bytes()? {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "target entity task belongs to another original source fence",
                ));
            }
            resume_fenced(self, &system)?;
        } else {
            let targets = acquire(self, &task)?;
            // System owner intent is durable before the first new target file.
            install_preparation_bytes(&system, &self.root.join(TASK), &task.bytes()?)?;
            self.backup_preparation = Some(match spawn(self, task.clone(), system, targets) {
                Ok(worker) => worker,
                Err(error) => task
                    .selection()
                    .clone()
                    .idle(error, BackupPreparationPhase::Blocked),
            });
        }
        self.backup_preparation_status()
            .ok_or_else(|| invalid("target image task has no observable worker"))
    }
}

pub(super) fn acquire(engine: &Engine, task: &Task) -> Result<Vec<Target>> {
    // The only exception to ordinary require_write is this narrow, source-fence
    // capability. No raw SYSTEM file can take this branch. Original all-member
    // seals, State and prefix are checked before reopening any target pathname.
    task.require_current(engine)?;
    engine.check_batch_scope(&task.selection().first().request)?;
    if let Some(worker) = &engine.backup_preparation
        && !worker.targets.is_empty()
    {
        let jobs = task.selection().jobs();
        if worker.job.request.operation_id != task.selection().first().request.operation_id
            || worker.targets.len() != jobs.len()
        {
            return Err(invalid(
                "held Backup target writers belong to another original task",
            ));
        }
        for (target, job) in worker.targets.iter().zip(jobs) {
            named_target(&target.root, job)?;
            target.recheck_writer()?;
            if target.root.path() != job.request.target_paths[0].as_path() {
                return Err(invalid("held Backup target order or original path changed"));
            }
        }
        return worker.targets.iter().map(Target::retain).collect();
    }
    let roots = task
        .selection()
        .jobs()
        .iter()
        .map(|job| DiskRoot::open(&job.request.target_paths[0]))
        .collect::<Result<Vec<_>>>()?;
    engine.check_batch_targets(&roots)?;
    let mut targets = Vec::with_capacity(roots.len());
    for (root, job) in roots.into_iter().zip(task.selection().jobs()) {
        named_target(&root, job)?;
        let lock = AdvisoryLock::directory(&root.directory(root.path())?).map_err(fs_error)?;
        if PhysicalBinding::of(&root)? != job.target {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "fenced target directory changed",
            ));
        }
        targets.push(Target::new(root, lock));
    }
    Ok(targets)
}

/// Called only after Engine recovery admitted the original source State/fence.
/// No current task or partial fence is inferred from an old progress cache.
pub(in crate::engine::backup_preparation) fn resume_fenced(
    engine: &mut Engine,
    system: &DiskRoot,
) -> Result<bool> {
    if super::registration::resume(engine, system)? {
        return Ok(true);
    }
    if !preparation_path_exists(system, &system.path().join(TASK))? {
        return Ok(false);
    }
    let task = Task::read(system)?;
    let resumed = (|| {
        let targets = acquire(engine, &task)?;
        spawn(
            engine,
            task.clone(),
            DiskRoot::open(system.path())?,
            targets,
        )
    })();
    engine.backup_preparation = Some(match resumed {
        Ok(worker) => worker,
        Err(error) => task
            .selection()
            .clone()
            .idle(error, BackupPreparationPhase::Blocked),
    });
    Ok(true)
}

#[derive(Serialize)]
struct Acceptance<'a> {
    format: &'static str,
    source_fence_hash: &'a str,
    task_hash: &'a str,
    node: &'a BackupPreparedTopologyNode,
    physical: &'a PhysicalBinding,
    image_manifest_hash: String,
    whole_files: usize,
    directory_repositories: usize,
    #[serialization(with = "storage_contract::u64_string")]
    bodies: u64,
    #[serialization(with = "storage_contract::u64_string")]
    body_bytes: u64,
}
fn acceptance(task: &Task, task_hash: &str, index: usize, image: &Image<'_>) -> Result<Vec<u8>> {
    bounded(&Acceptance {
        format: FORMAT,
        source_fence_hash: &task.source_fence_hash,
        task_hash,
        node: &task.evidence.task.topology.nodes[index],
        physical: &task.selection().jobs()[index].target,
        image_manifest_hash: image.manifest_hash(),
        whole_files: image.file_units,
        directory_repositories: image.directory_repositories,
        bodies: image.bodies,
        body_bytes: image.body_bytes,
    })
}

pub(super) fn with_image<T>(
    task: &Task,
    target: &Target,
    index: usize,
    stop: &AtomicBool,
    action: impl FnOnce(&Image<'_>) -> Result<T>,
) -> Result<T> {
    let job = &task.selection().jobs()[index];
    named_target(&target.root, job)?;
    target.recheck_writer()?;
    let state = snapshot::read(&target.root, job, stop)?;
    if object_hash("backup-prepared-state", state.state())? != task.evidence.state_hash() {
        return Err(invalid("target image snapshot differs from original fence"));
    }
    let image = Image::capture(
        &target.root,
        &state,
        &task.evidence.task.topology.nodes[index].source_prefix,
        &task.source_fence_hash,
        stop,
    )?;
    if image.file_units != task.evidence.report.whole_file_units_per_target
        || image.directory_repositories != task.evidence.report.directory_repositories_per_target
        || image.bodies != job.total_files
        || image.body_bytes != job.total_bytes
    {
        return Err(invalid(
            "target image is not the entire original supported unit",
        ));
    }
    let result = action(&image)?;
    named_target(&target.root, job)?;
    target.recheck_writer()?;
    Ok(result)
}

fn run(task: &Task, targets: &[Target], progress: &Progress) -> Result<()> {
    let stop = progress.stop.as_ref();
    let roots = targets
        .iter()
        .map(|target| &target.root)
        .collect::<Vec<_>>();
    let observed = super::verify_at(&task.evidence.task, &roots, stop)?;
    if bounded(&observed)? != bounded(&task.evidence.report)? {
        return Err(invalid("fenced original cutover materials changed"));
    }
    // Complete topology preflight before the first image or acknowledgement.
    for (index, target) in targets.iter().enumerate() {
        with_image(task, target, index, stop, |image| {
            image.require_capacity(&target.root, stop)
        })?;
    }
    let intent = task.bytes()?;
    let hash = object_hash(FORMAT, task)?;
    let mut acceptances = Vec::with_capacity(targets.len());
    for (index, target) in targets.iter().enumerate() {
        preparation_checkpoint(stop)?;
        progress.update( |status| {
            status.phase = BackupPreparationPhase::Copying;
            status.detail = format!(
                "installing independent target entity image {}/{}; source remains stopped and original bytes retained",
                index + 1,
                targets.len()
            );
        })?;
        install_preparation_bytes(&target.root, &target.root.path().join(INTENT), &intent)?;
        let accepted = with_image(task, target, index, stop, |image| {
            image.install(&target.root, stop)?;
            acceptance(task, &hash, index, image)
        })?;
        install_preparation_bytes(
            &target.root,
            &target.root.path().join(ACCEPTANCE),
            &accepted,
        )?;
        acceptances.push(accepted);
        let completed = index as u64 + 1;
        progress.update(|status| {
            status.completed_files = task.selection().first().total_files * completed;
            status.completed_bytes = task.selection().first().total_bytes * completed;
        })?;
    }
    progress.update( |status| {
        status.phase = BackupPreparationPhase::Verifying;
        status.detail = "independently rereading every installed image before whole-target completion; no authority publication".into();
    })?;
    for (index, target) in targets.iter().enumerate() {
        with_image(task, target, index, stop, |image| {
            image.verify(&target.root, stop)?;
            if target
                .root
                .read_bytes(&target.root.path().join(ACCEPTANCE), MAX_TASK)?
                != acceptance(task, &hash, index, image)?
            {
                return Err(invalid("target image lacks its exact durable acceptance"));
            }
            Ok(())
        })?;
    }
    let complete = completion(task, &hash, &acceptances)?;
    for (target, job) in targets.iter().zip(task.selection().jobs()) {
        preparation_checkpoint(stop)?;
        named_target(&target.root, job)?;
        target.recheck_writer()?;
        install_preparation_bytes(&target.root, &target.root.path().join(COMPLETE), &complete)?;
    }
    for (target, job) in targets.iter().zip(task.selection().jobs()) {
        named_target(&target.root, job)?;
        target.recheck_writer()?;
    }
    progress.update( |status| {
        status.phase = BackupPreparationPhase::TargetEntitiesPrepared;
        status.completed_files = status.total_files;
        status.completed_bytes = status.total_bytes;
        status.detail = "all isolated whole-unit entity images installed and verified; source write fence remains, active target registry/placement and final authority are not implemented".into();
    })
}

fn spawn(engine: &Engine, task: Task, system: DiskRoot, targets: Vec<Target>) -> Result<Worker> {
    let locks = engine.retain_writer_locks()?;
    let mut initial = task.selection().status();
    initial.detail =
        "recovering exact source-fenced installation; no active target or source deletion".into();
    let frozen = task.clone();
    task.evidence
        .task
        .scoped
        .objects
        .native
        .selection
        .worker(initial)
        .spawn(
            locks,
            system,
            "zxs-backup-target-entities",
            move |progress| {
                progress.finish(run(&frozen, &targets, progress));
            },
        )
}

fn completion(task: &Task, hash: &str, acceptances: &[Vec<u8>]) -> Result<Vec<u8>> {
    #[derive(Serialize)]
    struct Completion<'a> {
        format: &'static str,
        source_fence_hash: &'a str,
        task_hash: &'a str,
        target_acceptance_hashes: Vec<String>,
    }
    bounded(&Completion {
        format: FORMAT,
        source_fence_hash: &task.source_fence_hash,
        task_hash: hash,
        target_acceptance_hashes: acceptances
            .iter()
            .map(|bytes| blake3::hash(bytes).to_hex().to_string())
            .collect(),
    })
}

/// Require all old entity acceptances and their identical complete-set marker.
/// No decision caller may infer image completion from a progress phase alone.
pub(super) fn verify_installed(task: &Task, targets: &[Target], stop: &AtomicBool) -> Result<()> {
    let binding = |_: &AtomicBool| Ok(None);
    let mut encodings = encoding_memo::Transaction::new(None, &binding);
    verify_installed_with_encodings(task, targets, stop, &mut encodings)
}

pub(super) fn verify_installed_with_encodings(
    task: &Task,
    targets: &[Target],
    stop: &AtomicBool,
    encodings: &mut encoding_memo::Transaction<'_>,
) -> Result<()> {
    let roots = targets
        .iter()
        .map(|target| &target.root)
        .collect::<Vec<_>>();
    let observed = super::verify_at_with_encodings(&task.evidence.task, &roots, stop, encodings)?;
    if bounded(&observed)? != bounded(&task.evidence.report)? {
        return Err(invalid(
            "installed entities extend different original cutover inputs",
        ));
    }
    let hash = object_hash(FORMAT, task)?;
    let intent = task.bytes()?;
    let mut acceptances = Vec::with_capacity(targets.len());
    for (index, target) in targets.iter().enumerate() {
        let accepted = with_image(task, target, index, stop, |image| {
            image.verify(&target.root, stop)?;
            acceptance(task, &hash, index, image)
        })?;
        if target
            .root
            .read_bytes(&target.root.path().join(INTENT), MAX_INTENT)?
            != intent
            || target
                .root
                .read_bytes(&target.root.path().join(ACCEPTANCE), MAX_TASK)?
                != accepted
        {
            return Err(invalid(
                "image acceptance is absent or differs from original task",
            ));
        }
        acceptances.push(accepted);
    }
    let complete = completion(task, &hash, &acceptances)?;
    for (target, job) in targets.iter().zip(task.selection().jobs()) {
        named_target(&target.root, job)?;
        target.recheck_writer()?;
        if target
            .root
            .read_bytes(&target.root.path().join(COMPLETE), MAX_TASK)?
            != complete
        {
            return Err(invalid("target image lacks the exact complete-set marker"));
        }
    }
    Ok(())
}
