use crate::{disk::*, *};
use json_codec::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

mod config;
pub use config::ValidatedConfig;
mod planning;
pub(crate) use planning::PlanningContext;
mod index;
mod integrity_scrub;
pub use integrity_scrub::{
    IntegrityScrubArea, IntegrityScrubBatch, IntegrityScrubBatchAdmission,
    IntegrityScrubCancellation, IntegrityScrubCompletion, IntegrityScrubIssue,
    IntegrityScrubOutcome, IntegrityScrubPlan, IntegrityScrubReport, IntegrityScrubSchedule,
    IntegrityScrubTarget,
};
mod observations;
mod operations;
pub use observations::{DiskObservation, DiskObservationPlan, MetadataMemberProbe};
pub use operations::{OperationReadPlan, OperationRecord};
pub(crate) mod provenance;
mod publication;
pub(crate) use publication::DetachedPublication;
mod workspace_commit;
pub use workspace_commit::{PreparedWorkspaceCommit, WorkspaceCommitPreparation};
mod reads;
pub use reads::{ContentReadPlan, RangeContent, WorkspaceReadPlan};
mod recovery;
use recovery::RecoveryMode;
mod recovery_checkpoint;
mod workspace_budget;
mod workspace_limits;
pub use workspace_limits::RetainedWorkspaceLimits;
mod workspace_storage;
use config::{DiskLockPolicy, acquire_locks};

/// Nonfatal checkpoint failures from the latest recovery invocation.
#[derive(Debug, Default)]
pub struct RecoveryWarnings {
    pub checkpoint_load: Option<Error>,
    pub checkpoint_save: Option<Error>,
}

pub struct Engine {
    pub(crate) root: PathBuf,
    pub(crate) config: ValidatedConfig,
    pub(crate) state: Arc<State>,
    pub(crate) workspaces: BTreeMap<String, Workspace>,
    workspace_budget: workspace_budget::WorkspaceBudget,
    pub(crate) blocked: bool,
    pub(crate) snapshot: Option<String>,
    published_state_base: Option<Arc<publication::PublishedStateBase>>,
    pub(crate) metadata_integrity: Arc<provenance::MetadataIntegrity>,
    integrity_scrub_schedule: Option<std::sync::Weak<integrity_scrub::ScheduleState>>,
    pub(crate) next_slot: u64,
    pub(crate) journals: BTreeMap<String, crate::journal::JournalWriter>,
    pub(crate) reads: Arc<Mutex<BTreeMap<String, usize>>>,
    pub(crate) upload_preparations: Arc<Mutex<BTreeSet<String>>>,
    commit_preparation: Arc<std::sync::atomic::AtomicBool>,
    history_query_proof: Mutex<index::QueryProof>,
    recovery_warnings: RecoveryWarnings,
    _locks: Vec<File>,
    #[cfg(feature = "fault-injection")]
    pub(crate) fault: Option<String>,
}
pub struct ReadLease {
    version: String,
    reads: Arc<Mutex<BTreeMap<String, usize>>>,
}
impl Drop for ReadLease {
    fn drop(&mut self) {
        let mut reads = self
            .reads
            .lock()
            .expect("invariant: read lease lock poisoned");
        let count = reads
            .get_mut(&self.version)
            .expect("invariant: every read lease has a registered count");
        assert!(
            *count > 0,
            "invariant: a live read lease has a positive count"
        );
        *count -= 1;
        if *count == 0 {
            reads.remove(&self.version);
        }
    }
}
pub struct OpenedContent {
    pub file: File,
    pub size: u64,
    pub content_hash: crate::ContentHash,
    pub lease: ReadLease,
}
pub(crate) struct Prepared {
    pub version: FileVersion,
    pub placement: Placement,
    pub source: PathBuf,
}

/// A real unpublished mutation plan; constructing it never acknowledges a
/// publication or writes a decision record.
pub(crate) enum MutationPlan {
    Existing(OperationResult),
    Prepared {
        next: Box<State>,
        prepared: Vec<Prepared>,
        result: OperationResult,
    },
}

impl Engine {
    /// Inject one failure at the journal I/O boundary for recovery testing.
    #[cfg(feature = "fault-injection")]
    pub fn inject_journal_fault_once(
        &mut self,
        slot: &str,
        fault: crate::JournalFault,
    ) -> Result<()> {
        let disk = self.disk(slot)?.disk_id.clone();
        self.journals
            .get_mut(&disk)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "metadata journal is unavailable",
                )
            })?
            .fault = Some(fault);
        Ok(())
    }

    pub(crate) fn append_journal(
        &mut self,
        disk: &DiskConfig,
        kind: crate::journal::RecordType,
        sequence: u64,
        candidate: &str,
        object: &str,
    ) -> Result<crate::journal::Receipt> {
        let journal = self
            .journals
            .get_mut(&disk.disk_id)
            .expect("invariant: writable metadata member owns a journal");
        let result = journal.append(disk, kind, sequence, candidate, object);
        if journal.requires_reopen() {
            self.blocked = true;
            self.metadata_integrity.invalidate();
        }
        result
    }

    pub(crate) fn append_recovered_journal(
        &mut self,
        disk: &DiskConfig,
        kind: crate::journal::RecordType,
        sequence: u64,
        candidate: &str,
        object: &str,
    ) -> Result<crate::journal::Receipt> {
        let journal = self
            .journals
            .get_mut(&disk.disk_id)
            .expect("invariant: writable metadata member owns a journal");
        let result = journal.append_recovered(disk, kind, sequence, candidate, object);
        if journal.requires_reopen() {
            self.blocked = true;
            self.metadata_integrity.invalidate();
        }
        result
    }

    /// Detached filesystem work must keep the same OS lock descriptions alive
    /// until completion or cancellation, even if its original Engine is dropped.
    pub(crate) fn retain_writer_locks(&self) -> Result<Vec<File>> {
        Ok(self
            ._locks
            .iter()
            .map(File::try_clone)
            .collect::<std::io::Result<Vec<_>>>()?)
    }

    /// Candidate membership is immutable and must never shrink with availability.
    pub(crate) fn verify_candidate_membership(
        &self,
        candidate: &crate::journal::Candidate,
    ) -> Result<()> {
        self.metadata_integrity
            .observe(self.config.verify_candidate_membership(candidate))
    }

    pub fn initialize(root: &Path, config: Config) -> Result<Self> {
        Self::initialize_with_workspace_limits(root, config, None)
    }

    /// Set restart-only retained-evidence limits without changing disk configuration.
    pub fn initialize_with_workspace_limits(
        root: &Path,
        config: Config,
        workspace_limits: Option<RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        let config = ValidatedConfig::try_from(config)?.with_workspace_limits(workspace_limits)?;
        Self::initialize_validated(root, config)
    }

    /// Initialize an admitted configuration, retaining its restart-only policy.
    ///
    /// # Errors
    /// Returns filesystem, disk identity, canonical root overlap or initialization errors.
    pub fn initialize_validated(root: &Path, config: ValidatedConfig) -> Result<Self> {
        if root.join("config.json").try_exists()? {
            return Err(Error::new(
                storage_contract::ErrorCode::AlreadyInitialized,
                "configuration already exists; use open/recovery",
            ));
        }
        private_dir(root)?;
        let root = existing_private_dir(root)?.path().to_path_buf();
        let mut roots = Vec::with_capacity(config.disks.len());
        for disk in &config.disks {
            private_dir(&disk.root)?;
            let path = disk.root.canonicalize()?;
            if path.join("disk.json").try_exists()? {
                return Err(Error::new(
                    storage_contract::ErrorCode::DiskAlreadyEnrolled,
                    "disk already contains a managed identity",
                ));
            }
            roots.push(path);
        }
        let config = config.with_disk_roots(roots)?;
        let mut physical = BTreeSet::new();
        for d in &config.disks {
            let seal = identity(&config, d)?;
            if !physical.insert(seal.physical_device.clone()) {
                return Err(Error::new(
                    storage_contract::ErrorCode::DiskNotIndependent,
                    "two slots reference one physical disk",
                ));
            }
        }
        let locks = acquire_locks(&root, &config, DiskLockPolicy::NewPoolRoots)?;
        for d in &config.disks {
            let seal = identity(&config, d)?;
            write_json(&d.root.join("disk.json"), &seal, false)?;
            write_atomic(&d.root.join("README.txt"),b"ZXS disk format 2. Ordinary full files are stored below histories/. Use zxs-rescue offline-check/offline-export before recovery. Development disks are not physical redundancy evidence.\n",false)?;
            for p in [
                "histories",
                "metadata/application-log",
                "metadata/objects",
                "staging",
                "quarantine",
            ] {
                private_dir(&d.root.join(p))?;
            }
            sync_dir(&d.root)?;
        }
        // Every payload disk retains the fixed membership anchor for offline proofs.
        for d in &config.disks {
            write_json(
                &d.root.join("metadata/pool-membership/config.json"),
                &config,
                false,
            )?;
        }
        write_json(&root.join("config.json"), &config, false)?;
        private_dir(&root.join("workspaces"))?;
        let state = State {
            topology_generation: 1,
            ..State::default()
        };
        let journals = config
            .disks
            .iter()
            .filter(|disk| disk.metadata_member)
            .map(|disk| {
                Ok((
                    disk.disk_id.clone(),
                    crate::journal::JournalWriter::create(disk)?,
                ))
            })
            .collect::<Result<_>>()?;
        let engine = Self {
            root,
            config,
            state: Arc::new(state),
            workspaces: BTreeMap::new(),
            workspace_budget: workspace_budget::WorkspaceBudget::default(),
            blocked: false,
            snapshot: None,
            published_state_base: None,
            metadata_integrity: Arc::new(provenance::MetadataIntegrity::default()),
            integrity_scrub_schedule: None,
            next_slot: 1,
            journals,
            reads: Arc::new(Mutex::new(BTreeMap::new())),
            upload_preparations: Arc::new(Mutex::new(BTreeSet::new())),
            commit_preparation: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            history_query_proof: Mutex::new(index::QueryProof::default()),
            recovery_warnings: RecoveryWarnings::default(),
            _locks: locks,
            #[cfg(feature = "fault-injection")]
            fault: None,
        };
        engine.rebuild_index()?;
        Ok(engine)
    }
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_with_workspace_limits(root, None)
    }

    pub fn open_with_workspace_limits(
        root: &Path,
        workspace_limits: Option<RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        Self::open_mode(root, RecoveryMode::CompletePending, workspace_limits)
    }

    /// Offline recovery entry. Unpublished candidates are never advanced here.
    pub fn open_for_recovery(root: &Path) -> Result<Self> {
        Self::open_for_recovery_with_workspace_limits(root, None)
    }

    pub fn open_for_recovery_with_workspace_limits(
        root: &Path,
        workspace_limits: Option<RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        Self::open_mode(root, RecoveryMode::PreservePending, workspace_limits)
    }

    fn open_mode(
        root: &Path,
        mode: RecoveryMode,
        workspace_limits: Option<RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        let root = existing_private_dir(root)?.path().to_path_buf();
        let config = ValidatedConfig::try_from(read_json::<Config>(&root.join("config.json"))?)?
            .with_workspace_limits(workspace_limits)?;
        let mut engine = Self::prepare_workspace_recovery(root, config)?;
        engine.recover_mode(mode)?;
        engine.load_workspaces()?;
        engine.rebuild_index()?;
        Ok(engine)
    }

    /// Admission preflight is read-only apart from writer lock files. Keep this
    /// same Engine and its locks through replay and authoritative reconciliation.
    fn prepare_workspace_recovery(root: PathBuf, config: ValidatedConfig) -> Result<Self> {
        let locks = acquire_locks(&root, &config, DiskLockPolicy::AvailableMembers)?;
        let mut engine = Self {
            root,
            config,
            state: Arc::new(State {
                topology_generation: 1,
                ..State::default()
            }),
            workspaces: BTreeMap::new(),
            workspace_budget: workspace_budget::WorkspaceBudget::default(),
            blocked: false,
            snapshot: None,
            published_state_base: None,
            metadata_integrity: Arc::new(provenance::MetadataIntegrity::default()),
            integrity_scrub_schedule: None,
            next_slot: 1,
            journals: BTreeMap::new(),
            reads: Arc::new(Mutex::new(BTreeMap::new())),
            upload_preparations: Arc::new(Mutex::new(BTreeSet::new())),
            commit_preparation: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            history_query_proof: Mutex::new(index::QueryProof::default()),
            recovery_warnings: RecoveryWarnings::default(),
            _locks: locks,
            #[cfg(feature = "fault-injection")]
            fault: None,
        };
        engine.seed_workspace_recovery()?;
        Ok(engine)
    }
    /// Explicit recovery after loss of the system disk. Never initializes data disks.
    pub fn restore_from_disks(root: &Path, disks: &[PathBuf]) -> Result<Self> {
        Self::restore_from_disks_with_workspace_limits(root, disks, None)
    }

    pub fn restore_from_disks_with_workspace_limits(
        root: &Path,
        disks: &[PathBuf],
        workspace_limits: Option<RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        if root.join("config.json").try_exists()? {
            return Err(Error::new(
                storage_contract::ErrorCode::AlreadyInitialized,
                "restore requires a new system state directory",
            ));
        }
        if disks.is_empty() {
            return Err(Error::new(
                storage_contract::ErrorCode::MetadataQuorumUnavailable,
                "restore requires trusted old metadata members",
            ));
        }
        let mut configs = BTreeMap::<String, (ValidatedConfig, usize)>::new();
        let mut seals = BTreeMap::new();
        for path in disks {
            let seal: DiskSeal = read_json(&path.join("disk.json"))?;
            let config = ValidatedConfig::try_from(read_json::<Config>(
                &path.join("metadata/pool-membership/config.json"),
            )?)?;
            let mut member = config
                .disks
                .iter()
                .find(|disk| disk.disk_id == seal.disk_id && disk.metadata_member)
                .cloned()
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::ConfigConflict,
                        "recovery input is not a metadata member of its saved configuration",
                    )
                })?;
            let path = path.canonicalize()?;
            member.root = path.clone();
            validate_seal(&config, &member)?;
            let hash = object_hash("pool-config", &config)?;
            configs
                .entry(hash)
                .and_modify(|(_, n)| *n += 1)
                .or_insert((config, 1));
            if seals.insert(seal.disk_id, path).is_some() {
                return Err(Error::new(
                    storage_contract::ErrorCode::DiskNotIndependent,
                    "duplicate disk id in recovery input",
                ));
            }
        }
        let mut agreed = configs
            .into_values()
            .filter(|(c, n)| *n >= c.metadata_quorum())
            .map(|(c, _)| c);
        let config = agreed.next().ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ConfigConflict,
                "a configured metadata majority does not agree on pool configuration",
            )
        })?;
        if agreed.next().is_some()
            || seals.keys().any(|id| {
                !config
                    .disks
                    .iter()
                    .any(|disk| disk.metadata_member && &disk.disk_id == id)
            })
        {
            return Err(Error::new(
                storage_contract::ErrorCode::ConfigConflict,
                "recovery inputs identify more than one pool configuration or membership",
            ));
        }
        let roots = config
            .disks
            .iter()
            .map(|disk| seals.get(&disk.disk_id).unwrap_or(&disk.root).clone())
            .collect();
        let config = config.with_disk_roots(roots)?;
        for member in config
            .disks
            .iter()
            .filter(|disk| seals.contains_key(&disk.disk_id))
        {
            validate_seal(&config, member)?;
        }
        let config = config.with_workspace_limits(workspace_limits)?;
        private_dir(root)?;
        let root = existing_private_dir(root)?.path().to_path_buf();
        let mut engine = Self::prepare_workspace_recovery(root, config)?;
        // A quota failure must not publish a system configuration or replay any
        // journal repair. Preflight above retains the locks across publication.
        write_json(&engine.root.join("config.json"), &engine.config, false)?;
        engine.recover_mode(RecoveryMode::CompletePending)?;
        engine.load_workspaces()?;
        engine.rebuild_index()?;
        Ok(engine)
    }
    pub(crate) fn require_recovery_locks(&self) -> Result<()> {
        if self._locks.len() != self.config.disks.len() + 1 {
            return Err(Error::new(
                storage_contract::ErrorCode::RecoveryRequired,
                "recovery requires every original disk and its exclusive OS writer lock",
            ));
        }
        if self.journals.len()
            != self
                .config
                .disks
                .iter()
                .filter(|disk| disk.metadata_member)
                .count()
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "reopen recovery to acquire every metadata journal",
            ));
        }
        for disk in &self.config.disks {
            self.metadata_integrity.validate_seal(&self.config, disk)?;
        }
        Ok(())
    }
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn state(&self) -> &State {
        self.state.as_ref()
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Drain nonfatal startup diagnostics for reporting once at the application boundary.
    pub fn take_recovery_warnings(&mut self) -> RecoveryWarnings {
        std::mem::take(&mut self.recovery_warnings)
    }
    pub fn is_read_only(&self) -> bool {
        self.blocked
    }
    /// Borrow the unseen event suffix from admitted, publication-ordered State.
    /// Cold admission rejects decreasing sequences; writers only append newer
    /// publication slots, and recovery preserves the admitted ordinal order.
    pub fn events_after(&self, after: u64) -> impl ExactSizeIterator<Item = &Event> {
        let rank = self
            .state
            .events
            .partition_point(|event| event.sequence <= after);
        self.state.events[rank..].iter()
    }
    pub fn auth_state(&self) -> Option<json_codec::Value> {
        self.state.control_auth.clone()
    }
    /// Borrow the current authoritative control snapshot while the caller holds
    /// its engine guard. Owned recovery consumers should use `auth_state`.
    pub fn auth_state_ref(&self) -> Option<&json_codec::Value> {
        self.state.control_auth.as_ref()
    }
    /// Trusted local control-plane hook; this method is never a public route.
    pub fn persist_auth_state(
        &mut self,
        snapshot: json_codec::Value,
        principal: &str,
        operation: OperationId,
    ) -> Result<()> {
        if !snapshot.is_object() || canonical_json(&snapshot)?.len() > 4 * 1024 * 1024 {
            return Err(Error::new(
                storage_contract::ErrorCode::AuthStateInvalid,
                "authentication snapshot is not a bounded object",
            ));
        }
        let digest = object_hash("auth-state", &snapshot)?;
        self.mutate(
            "auth.control-state",
            principal,
            operation,
            json!({"auth_state_digest":digest}),
            |_, state, _| {
                state.control_auth = Some(snapshot);
                Ok(json!({"auth_state_digest":digest}))
            },
        )?;
        Ok(())
    }
    /// Own a workspace snapshot for planning that outlives a shared engine borrow.
    pub fn workspace(&self, id: &str) -> Result<Workspace> {
        self.workspace_ref(id).cloned()
    }
    /// Inspect a workspace without copying its staged-file and namespace records.
    /// The view cannot outlive the engine borrow or span a mutation.
    pub fn workspace_ref(&self, id: &str) -> Result<&Workspace> {
        self.workspaces.get(id).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::WorkspaceNotFound,
                "workspace does not exist",
            )
        })
    }
    /// Deterministic injection only, never enabled by network or environment input.
    #[cfg(feature = "fault-injection")]
    pub fn inject_fault_once(&mut self, point: &str) {
        self.fault = Some(point.to_string());
    }
    #[cfg(feature = "fault-injection")]
    pub(crate) fn failpoint(&mut self, point: &str) -> Result<()> {
        if self.fault.as_deref() == Some(point) {
            self.fault = None;
            return Err(Error::new(
                storage_contract::ErrorCode::InjectedFailure,
                format!("interrupted at {point}"),
            ));
        }
        Ok(())
    }
    pub(crate) fn trusted_evidence_members(&self) -> Vec<DiskConfig> {
        self.members()
            .into_iter()
            .filter(|disk| {
                self.metadata_integrity
                    .observe(validate_seal(&self.config, disk))
                    .is_ok()
            })
            .collect()
    }
    pub(crate) fn members(&self) -> Vec<DiskConfig> {
        self.planning_context().members()
    }
    pub(crate) fn disk(&self, slot: &str) -> Result<&DiskConfig> {
        self.planning_context().disk(slot)
    }
    pub(crate) fn route(&self, primary: &str, copies: u32) -> Result<Vec<String>> {
        self.planning_context().route(primary, copies)
    }
    pub(crate) fn require_no_commit_preparation(&self) -> Result<()> {
        if self
            .commit_preparation
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::CommitPreparationBusy,
                "workspace payload preparation temporarily fences other writes",
            )
            .retryable());
        }
        Ok(())
    }
    pub(crate) fn require_write(&self) -> Result<()> {
        self.require_no_commit_preparation()?;
        if self.blocked {
            return Err(Error::new(
                storage_contract::ErrorCode::RecoveryRequired,
                "an unresolved decision or unavailable metadata member blocks writes",
            )
            .retryable());
        }
        for d in self.members() {
            self.metadata_integrity
                .observe(validate_seal(&self.config, &d))?;
        }
        Ok(())
    }

    /// An actor is immutable audit/idempotency input, not a role or credential.
    /// Authentication is performed by the server before calling this trusted API.
    pub(crate) fn require_actor(principal: &str) -> Result<()> {
        if principal.is_empty() {
            return Err(Error::new(
                storage_contract::ErrorCode::Unauthenticated,
                "operation actor is required",
            ));
        }
        Ok(())
    }

    pub(crate) fn repository_for_history(&self, history: &str) -> Result<&Repository> {
        self.planning_context().repository_for_history(history)
    }
    pub(crate) fn repository_for_branch(&self, branch: &str) -> Result<&Repository> {
        self.planning_context().repository_for_branch(branch)
    }
    pub(crate) fn mutate<F>(
        &mut self,
        kind: &str,
        principal: &str,
        operation_id: OperationId,
        input: json_codec::Value,
        change: F,
    ) -> Result<OperationResult>
    where
        F: FnOnce(&Self, &mut State, &mut Vec<Prepared>) -> Result<json_codec::Value>,
    {
        self.mutate_digest(kind, principal, operation_id, input, None, change)
    }
    pub(crate) fn mutate_digest<F>(
        &mut self,
        kind: &str,
        principal: &str,
        operation_id: OperationId,
        input: json_codec::Value,
        digest_override: Option<String>,
        change: F,
    ) -> Result<OperationResult>
    where
        F: FnOnce(&Self, &mut State, &mut Vec<Prepared>) -> Result<json_codec::Value>,
    {
        match self.plan_mutation_digest(
            kind,
            principal,
            operation_id,
            input,
            digest_override,
            change,
        )? {
            MutationPlan::Existing(result) => Ok(result),
            MutationPlan::Prepared {
                next,
                prepared,
                result,
            } => {
                self.persist(*next, prepared, result.clone())?;
                Ok(result)
            }
        }
    }
    pub(crate) fn plan_mutation_digest<F>(
        &self,
        kind: &str,
        principal: &str,
        operation_id: OperationId,
        input: json_codec::Value,
        digest_override: Option<String>,
        change: F,
    ) -> Result<MutationPlan>
    where
        F: FnOnce(&Self, &mut State, &mut Vec<Prepared>) -> Result<json_codec::Value>,
    {
        Self::require_actor(principal)?;
        let digest = digest_override.map(Ok).unwrap_or_else(|| {
            object_hash(
                "request",
                &json!({"kind":kind,"principal":principal,"body":input}),
            )
        })?;
        if let Some(result) = self.state.operations.get(&operation_id) {
            if result.request_digest == digest
                && self.operation_owner(operation_id)?.as_deref() == Some(principal)
            {
                return self.operation(operation_id).map(MutationPlan::Existing);
            }
            return Err(Error::new(
                storage_contract::ErrorCode::IdempotencyKeyReused,
                "operation id is already bound to different input",
            ));
        }
        self.require_write()?;
        self.planning_context().plan_new_mutation(
            kind,
            principal,
            operation_id,
            digest,
            |_, next, prepared| change(self, next, prepared),
        )
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/engine/payload_measurement.rs"]
mod payload_measurement;
