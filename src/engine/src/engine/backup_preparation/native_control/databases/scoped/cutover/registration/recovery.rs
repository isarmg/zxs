//! A decided frozen read view, deliberately separate from active membership.
//! Detached reads require an independently retained decision hash and the entire
//! original selected set. No old source/SYSTEM path is opened, repaired or used.
use super::*;
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupRegistryDecisionHash};

/// Observation of actual native choice plus complete target readback. Not a
/// migration-success receipt or permission to release the source fence.
#[derive(Clone, Debug)]
pub struct BackupRegistryRecoveryReport {
    pub operation_id: OperationId,
    pub decision_hash: BackupRegistryDecisionHash,
    pub source_fence_hash: String,
    pub original_state_hash: String,
    pub original_configuration_hash: String,
    pub targets: Vec<BackupPreparedTopologyNode>,
}

/// Exact bytes and metadata from a frozen AVAILABLE version. Not a mutable file
/// handle and not a current namespace view; an empty range still authenticates
/// the whole selected body. No active State/private authentication is exposed.
#[derive(Clone, Debug)]
pub struct BackupRecoveredContent {
    pub decision_hash: BackupRegistryDecisionHash,
    pub target_slot_id: String,
    pub version: FileVersion,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

/// Holds every original selected target directory lock until dropped. It is a
/// recovery reader only: there is no conversion to DiskResources, Config,
/// Generation or an ordinary Engine reader/writer.
pub struct BackupDecidedTargets {
    decision: AdmittedBackupDecision,
    prepared: PreparedBackupRegistry,
    report: BackupRegistryRecoveryReport,
}
impl BackupDecidedTargets {
    /// A cached description of the successful open, not a freshness probe.
    pub fn report(&self) -> &BackupRegistryRecoveryReport {
        &self.report
    }

    /// Re-read every decided image and the exact prepared fanout on all
    /// targets. A completion file by itself never authorizes an active writer.
    pub fn inspect_writer_routes(&self) -> Result<BackupWriterRoutesReport> {
        Ok(
            writer_routes::observe(&self.prepared, &self.decision, &AtomicBool::new(false))?
                .report(),
        )
    }

    /// Observe actual unpublished native metadata from every independent target.
    /// Neither the snapshot hash nor this report is an active generation proof.
    pub fn inspect_generation_inputs(&self) -> Result<BackupGenerationPreparationReport> {
        generation_inputs::observe(&self.prepared, &self.decision, &AtomicBool::new(false))
    }

    /// Independently replay every target's normal native predecessor archive,
    /// including its exact original MembershipFence journals. No source/SYSTEM
    /// access, repair, new vote or writer activation is performed.
    pub fn inspect_generation_control(&self) -> Result<BackupGenerationControlReport> {
        generation_control::observe(&self.prepared, &self.decision, &AtomicBool::new(false))
    }

    /// Read the native Backup core from every target after independently
    /// reconstructing the original fenced archive and unpublished routing
    /// input. This report cannot be used to activate an ordinary Engine writer.
    pub fn inspect_generation_authority(&self) -> Result<BackupGenerationAuthorityReport> {
        generation_authority::observe(&self.prepared, &self.decision, &AtomicBool::new(false))
    }

    /// Reconstruct the original exact first candidate from each target's own
    /// archived source and native Backup core, then read and admit its complete
    /// snapshot. The returned metadata does not mean the candidate was accepted
    /// or published and cannot grant writer/source-unfence authority.
    pub fn inspect_first_generation_candidate(&self) -> Result<BackupFirstCandidateReport> {
        first_candidate::observe(&self.prepared, &self.decision, &AtomicBool::new(false))
    }

    /// Continue only this selected task's isolated native candidate bytes. The
    /// caller must authorize target writes. No source/SYSTEM is opened, no old
    /// receipt is replaced, and missing original authority/archive remains an
    /// error rather than permission to synthesize an active generation.
    pub fn resume_first_candidate_preparation(&mut self) -> Result<BackupFirstCandidateReport> {
        first_candidate::prepare(&self.prepared, &self.decision, &AtomicBool::new(false))
    }

    /// Explicit no-clobber writes of this original task's native authority
    /// material only. Missing fenced control bytes remain an error, not a new
    /// synthesized source. The caller must authorize target writes. No source
    /// root, active marker, journal, receipt or fence is changed.
    pub fn resume_authority_preparation(&mut self) -> Result<BackupGenerationAuthorityReport> {
        generation_authority::prepare(&self.prepared, &self.decision, &AtomicBool::new(false))
    }

    /// Explicit offline continuation of the already chosen task's isolated
    /// materials. An independently retained decision digest was required at
    /// open. No source/SYSTEM root is probed, no new choice is issued, and no
    /// active marker, receipt, writer or source-unfence is created. The caller
    /// must authorize these no-clobber target writes, not merely a read request.
    pub fn resume_generation_preparation(&mut self) -> Result<BackupGenerationPreparationReport> {
        let stop = AtomicBool::new(false);
        require_isolated(&self.prepared.targets)?;
        self.decision.verify_targets(&self.prepared, &stop)?;
        writer_routes::prepare(&self.prepared, &self.decision, &stop)?;
        generation_inputs::prepare(&self.prepared, &self.decision, &stop)
    }

    /// Resolve one complete file to the full explicitly selected set. The
    /// caller cannot supply a subset of its versions or choose a sibling as a
    /// linear next hop. These frozen addresses carry no I/O/write admission.
    pub fn file_writer_route(&self, history: &str, file: &str) -> Result<BackupFileWriterRoute> {
        validate_id(history, "history")?;
        validate_id(file, "file")?;
        writer_routes::observe(&self.prepared, &self.decision, &AtomicBool::new(false))?
            .file(history, file)
    }

    /// Resolve a directory repository, including its actual enclosing parent,
    /// to the same complete target set as its children and retained files.
    pub fn directory_writer_route(
        &self,
        history: &str,
        directory: &str,
    ) -> Result<BackupDirectoryWriterRoute> {
        validate_id(history, "history")?;
        crate::container_storage::validate_database_directory_id(directory)?;
        writer_routes::observe(&self.prepared, &self.decision, &AtomicBool::new(false))?
            .directory(history, directory)
    }
    /// Rechecks the complete target set and selected decision on every call.
    /// Content is then read from the selected image inode in one full hashing
    /// pass, buffering only this <=4 MiB range. No fallback to another target or
    /// an old source can hide a damaged requested copy.
    pub fn read_version_range(
        &self,
        slot: &str,
        version_id: &str,
        offset: u64,
        length: usize,
    ) -> Result<BackupRecoveredContent> {
        name_policy::validate_slot(slot)?;
        validate_id(version_id, "ver")?;
        if length > crate::container_storage::backup_entities::MAX_RECOVERY_RANGE {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "decided backup range exceeds 4 MiB",
            ));
        }
        let stop = AtomicBool::new(false);
        require_isolated(&self.prepared.targets)?;
        self.decision.verify_targets(&self.prepared, &stop)?;
        let index = self
            .prepared
            .projection
            .targets
            .iter()
            .position(|target| target.node.identity.slot_id == slot)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DiskNotFound,
                    "slot is not in the exact decided target set",
                )
            })?;
        let target = &self.prepared.targets[index];
        let (version, bytes) = installation::with_image(
            &self.prepared.task.installation,
            target,
            index,
            &stop,
            |image| image.read_version_range(&target.root, version_id, offset, length, &stop),
        )?;
        require_isolated(&self.prepared.targets)?;
        self.prepared.recheck_names()?;
        Ok(BackupRecoveredContent {
            decision_hash: self.decision.hash(),
            target_slot_id: slot.into(),
            version,
            offset,
            bytes,
        })
    }
}

/// Open the same complete, still-isolated target set after the original source
/// or SYSTEM becomes unavailable. `expected` must come from a previously saved
/// authenticated live report, not from these same target bytes. This function
/// does not establish external trust or install a runtime member. Hosts must
/// authorize the recovery reader and must not expose it as a normal SMB/HTTP
/// file path. Exact original path/device/inode bindings remain mandatory.
pub fn open_decided_backup_targets(
    paths: &[PathBuf],
    expected: &BackupRegistryDecisionHash,
) -> Result<BackupDecidedTargets> {
    if !(1..=8).contains(&paths.len()) {
        return Err(invalid(
            "decided recovery needs the entire original selected target set",
        ));
    }
    // Only explicit caller paths are opened. Untrusted intent paths never cause
    // arbitrary filesystem access or a probe of the original source members.
    let roots = paths
        .iter()
        .map(|path| DiskRoot::open(path))
        .collect::<Result<Vec<_>>>()?;
    let first = &roots[0];
    let decision = AdmittedBackupDecision::from_trusted_bytes(
        &first.read_bytes(
            &first.path().join(DIRECTORY).join(DECISION),
            MAX_DECISION_BYTES,
        )?,
        expected,
    )?;
    let installation = InstallationTask::read_target(first)?;
    decision.bind_source(&installation.source_fence_hash, &installation.evidence)?;
    if !installation
        .selection()
        .jobs()
        .iter()
        .map(|job| &job.request.target_paths[0])
        .eq(paths.iter())
    {
        return Err(invalid(
            "detached recovery cannot omit, reorder, substitute or rebind a target",
        ));
    }
    let mut identities = BTreeSet::new();
    let mut targets = Vec::with_capacity(roots.len());
    for (root, job) in roots.into_iter().zip(installation.selection().jobs()) {
        named_target(&root, job)?;
        let _lock = AdvisoryLock::directory(&root.directory(root.path())?).map_err(fs_error)?;
        let identity = PhysicalBinding::of(&root)?;
        if identity != job.target || !identities.insert((identity.device, identity.inode)) {
            return Err(invalid(
                "decided recovery target identity changed or repeats",
            ));
        }
        named_target(&root, job)?;
        targets.push(Target { root, _lock });
    }
    require_isolated(&targets)?;
    let stop = AtomicBool::new(false);
    let prepared = PreparedBackupRegistry::capture(
        Task {
            format: FORMAT.into(),
            installation,
        },
        targets,
        &stop,
    )?;
    decision.verify_targets(&prepared, &stop)?;
    let report = report(&decision, &prepared);
    Ok(BackupDecidedTargets {
        decision,
        prepared,
        report,
    })
}

pub(super) fn require_isolated(targets: &[Target]) -> Result<()> {
    for target in targets {
        // A future native activation must use its own admitted live reader. An
        // old frozen capability cannot bypass active lifecycle/retirement gates.
        for name in ["disk.json", "metadata"] {
            if preparation_path_exists(&target.root, &target.root.path().join(name))? {
                return Err(Error::new(
                    ErrorCode::UnsupportedOperation,
                    "decided recovery reader cannot open a runtime/admission target",
                ));
            }
        }
    }
    Ok(())
}
fn report(
    decision: &AdmittedBackupDecision,
    prepared: &PreparedBackupRegistry,
) -> BackupRegistryRecoveryReport {
    BackupRegistryRecoveryReport {
        operation_id: decision.operation(),
        decision_hash: decision.hash(),
        source_fence_hash: prepared.source_fence_hash().into(),
        original_state_hash: prepared.projection.original_state_hash.clone(),
        original_configuration_hash: prepared.projection.original_configuration_hash.clone(),
        targets: prepared
            .projection
            .targets
            .iter()
            .map(|target| target.node.clone())
            .collect(),
    }
}

impl Engine {
    /// Read the exact all-original-member choice and every complete target. This
    /// is the authenticated live source of a digest for detached recovery. It
    /// never selects missing votes, installs copies, or releases a native fence.
    /// The host must authenticate/authorize the original actor independently.
    pub fn inspect_backup_registry_decision(
        &mut self,
        operation: OperationId,
        principal: &str,
    ) -> Result<BackupRegistryRecoveryReport> {
        Self::require_actor(principal)?;
        self.require_no_commit_preparation()?;
        let system = DiskRoot::open(&self.root)?;
        let task = Task::read(&system)?;
        if task.installation.bytes()?
            != InstallationTask::current(self, operation, principal)?.bytes()?
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "decision inspection belongs to another actor or original task",
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
                "wait for the retained worker before decision inspection",
            ));
        }
        if let Some(worker) = self.backup_preparation.as_mut() {
            worker.join()?;
        }
        let (targets, choices) = runtime::acquire(self, &task)?;
        require_isolated(&targets)?;
        let stop = AtomicBool::new(false);
        let prepared = PreparedBackupRegistry::capture(task, targets, &stop)?;
        let decision = choices.observe_decision(&prepared, &stop)?;
        Ok(report(&decision, &prepared))
    }
}

impl Engine {
    /// Inspect the real first publication in the reserved native generation.
    /// Requires all original source members and every selected target, reads
    /// every actual routed body, and never repairs/writes or installs a serving
    /// generation. A Publication report is not migration-success authority.
    pub fn inspect_backup_first_publication(
        &mut self,
        operation: OperationId,
        principal: &str,
    ) -> Result<crate::BackupFirstPublicationReport> {
        Self::require_actor(principal)?;
        self.require_no_commit_preparation()?;
        let task = Task::read(&DiskRoot::open(&self.root)?)?;
        if task.installation.bytes()?
            != InstallationTask::current(self, operation, principal)?.bytes()?
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "first Backup Publication inspection belongs to another actor or task",
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
                "wait for the retained worker before first Publication inspection",
            ));
        }
        if let Some(worker) = self.backup_preparation.as_mut() {
            worker.join()?;
        }
        let (targets, choices) = runtime::acquire(self, &task)?;
        require_isolated(&targets)?;
        let stop = AtomicBool::new(false);
        let prepared = PreparedBackupRegistry::capture(task, targets, &stop)?;
        let decision = choices.observe_decision(&prepared, &stop)?;
        let (_, candidate) = first_candidate::read_verified(&prepared, &decision, &stop)?;
        choices.observe_first_publication(&prepared, &candidate, &stop)
    }

    /// Observe actual native ACCEPT/FINALIZE from every original member. This
    /// needs the original fenced Engine and complete selected target set; the
    /// detached target reader cannot manufacture these live acknowledgements.
    /// It never creates a journal, repairs bytes or returns migration success.
    pub fn inspect_backup_first_finalize(
        &mut self,
        operation: OperationId,
        principal: &str,
    ) -> Result<crate::BackupFirstFinalizeReport> {
        Self::require_actor(principal)?;
        self.require_no_commit_preparation()?;
        let system = DiskRoot::open(&self.root)?;
        let task = Task::read(&system)?;
        if task.installation.bytes()?
            != InstallationTask::current(self, operation, principal)?.bytes()?
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "first Backup FINALIZE inspection belongs to another actor or task",
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
                "wait for the retained worker before first-slot inspection",
            ));
        }
        if let Some(worker) = self.backup_preparation.as_mut() {
            worker.join()?;
        }
        let (targets, choices) = runtime::acquire(self, &task)?;
        require_isolated(&targets)?;
        let stop = AtomicBool::new(false);
        let prepared = PreparedBackupRegistry::capture(task, targets, &stop)?;
        let decision = choices.observe_decision(&prepared, &stop)?;
        let (_, candidate) = first_candidate::read_verified(&prepared, &decision, &stop)?;
        choices.observe_first_finalize(&prepared, &candidate, &stop)
    }
}
