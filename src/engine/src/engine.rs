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
mod event_read;
mod history_window;
mod published_read;
pub use event_read::EventPageReadPlan;
pub(crate) use history_window::HistoryCatalog;
pub use history_window::HistoryRoot;
pub use published_read::{FileHistoryReadView, PublishedReadView};
mod index;
pub use index::{IndexFailure, IndexFailureStage, IndexHealth};
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
pub use operations::{
    OperationLookup, OperationReadObservation, OperationReadPlan, OperationRecord,
};
pub(crate) mod provenance;
mod publication;
#[cfg(feature = "fault-injection")]
pub use publication::MemberFault;
pub(crate) use publication::{DetachedPublication, PublishedDependencyReuse};
pub use publication::{PublicationStageReport, StageIo, StageKind, StageTiming};
mod workspace_commit;
#[cfg(test)]
pub(crate) use workspace_commit::native_workspace_commit_completed;
pub use workspace_commit::{PreparedWorkspaceCommit, WorkspaceCommitPreparation};
mod reads;
pub use reads::{ContentReadPlan, RangeContent, WorkspaceReadPlan};
mod recovery;
use recovery::RecoveryMode;
mod recovery_checkpoint;
mod workspace_budget;
mod workspace_limits;
pub use workspace_limits::RetainedWorkspaceLimits;
mod source;
mod workspace_storage;
use config::acquire_locks;
pub(crate) use source::ReadSource;
pub use workspace_storage::{WorkspaceInventory, WorkspaceInventoryRecord};

/// The seal boundary that kept a recovery invocation from admitting evidence.
#[derive(Debug, PartialEq, Eq)]
pub enum RecoverySealContext {
    /// Revalidation on the admitted member's held root.
    Member { disk_id: String, slot_id: String },
    /// Aggregate barrier failure; the verifier does not identify a replica.
    Publication { candidate_hash: String },
}

/// A retained concrete cause, separate from recovery decisions and current health.
#[derive(Debug, error_definition::Error)]
#[error("recovery seal verification failed: {context:?}")]
pub struct RecoverySealFailure {
    context: RecoverySealContext,
    #[source]
    source: Error,
}

impl RecoverySealFailure {
    pub fn context(&self) -> &RecoverySealContext {
        &self.context
    }

    pub fn cause(&self) -> &Error {
        &self.source
    }
}

/// Handled failures from the latest recovery invocation, drained by the application.
#[derive(Debug, Default)]
pub struct RecoveryWarnings {
    /// At most one failure per configured metadata member and one publication barrier.
    pub seals: Vec<RecoverySealFailure>,
    pub checkpoint_load: Option<Error>,
    pub checkpoint_save: Option<Error>,
}

pub struct Engine {
    pub(crate) root: PathBuf,
    pub(crate) config: ValidatedConfig,
    pub(crate) disk_resources: Arc<DiskResources>,
    pub(crate) state: Arc<State>,
    pub(crate) workspaces: BTreeMap<String, Workspace>,
    workspace_budget: workspace_budget::WorkspaceBudget,
    pub(crate) blocked: bool,
    pub(crate) snapshot: Option<String>,
    published_state_base: Option<Arc<publication::PublishedStateBase>>,
    /// The admitted history catalog bound to the current `state` allocation.
    /// None until the first publication or recovery binds one.
    pub(crate) history_catalog: Option<HistoryCatalog>,
    pub(crate) metadata_integrity: Arc<provenance::MetadataIntegrity>,
    integrity_scrub_schedule: Option<std::sync::Weak<integrity_scrub::ScheduleState>>,
    pub(crate) next_slot: u64,
    pub(crate) journals: BTreeMap<String, crate::journal::JournalWriter>,
    pub(crate) reads: Arc<Mutex<BTreeMap<String, usize>>>,
    pub(crate) upload_preparations: Arc<Mutex<BTreeSet<String>>>,
    commit_preparation: Arc<std::sync::atomic::AtomicBool>,
    history_query_proof: Mutex<index::QueryProof>,
    /// Bounded, non-authoritative caches for sealed segment and locator reads.
    pub(crate) segment_caches: Arc<crate::segment_store::SegmentCaches>,
    /// The published operation-locator root; None only while the one-shot
    /// cold index from recovery is still attached to the resident log.
    pub(crate) operation_locator_root: Option<crate::metadata_map::Reference>,
    recovery_warnings: RecoveryWarnings,
    last_publication_stages: Option<Arc<PublicationStageReport>>,
    _locks: Vec<File>,
    #[cfg(feature = "fault-injection")]
    fault: Option<FaultInjection>,
    /// Member-local faults, consumed inside their matching stage tasks.
    #[cfg(feature = "fault-injection")]
    member_faults: Vec<publication::ArmedMemberFault>,
}
#[cfg(feature = "fault-injection")]
struct FaultInjection {
    point: String,
    action: FaultAction,
}
#[cfg(feature = "fault-injection")]
enum FaultAction {
    Fail,
    Pause(Arc<std::sync::Barrier>),
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
    pub source: ReadSource,
}

/// A real unpublished mutation plan; constructing it never acknowledges a
/// publication or writes a decision record.
pub(crate) enum MutationPlan {
    Existing(OperationResult),
    Prepared {
        next: Box<State>,
        prepared: Vec<Prepared>,
        namespace: Vec<crate::journal::PreparedMetadataObject>,
        result: OperationResult,
    },
}

/// Per-mutation preparation outputs beyond the next State: payload placements
/// and the transaction's freshly built immutable namespace objects, which
/// install with the snapshot before the decision.
#[derive(Default)]
pub(crate) struct MutationScratch {
    pub(crate) payloads: Vec<Prepared>,
    pub(crate) namespace: crate::namespace::NamespaceObjects,
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

    /// Apply the journal failure fence for every member journal a stage wrote.
    /// Equivalent to the per-append check in `append_journal`, evaluated once
    /// the whole stage has returned and all member tasks are joined.
    pub(crate) fn fence_failed_journals(&mut self, members: &[DiskConfig]) {
        if members.iter().any(|disk| {
            self.journals
                .get(&disk.disk_id)
                .is_some_and(crate::journal::JournalWriter::requires_reopen)
        }) {
            self.blocked = true;
            self.metadata_integrity.invalidate();
        }
    }

    /// Stage report of the most recent completed publication in this process;
    /// records what this engine did on these devices, not a throughput claim.
    pub fn last_publication_stages(&self) -> Option<Arc<PublicationStageReport>> {
        self.last_publication_stages.clone()
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
        let disk_resources = Arc::new(DiskResources::open(&config, false)?);
        let locks = acquire_locks(&root, &disk_resources)?;
        for d in &config.disks {
            let disk = disk_resources.get(d)?;
            disk.root
                .write_json(&d.root.join("disk.json"), &disk.seal, false)?;
            disk.root.write_atomic(&d.root.join("README.txt"),b"ZXS disk format 2. Ordinary full files are stored below histories/. Use zxs-rescue offline-check/offline-export before recovery. Development disks are not physical redundancy evidence.\n",false)?;
            for p in [
                "histories",
                "metadata/application-log",
                "metadata/objects",
                "staging",
                "quarantine",
            ] {
                disk.root.private_dir(&d.root.join(p))?;
            }
            disk.root.sync_dir(&d.root)?;
        }
        // Every payload disk retains the fixed membership anchor for offline proofs.
        for d in &config.disks {
            disk_resources.get(d)?.root.write_json(
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
                    crate::journal::JournalWriter::create_at(
                        &disk_resources.get(disk)?.root,
                        disk,
                    )?,
                ))
            })
            .collect::<Result<_>>()?;
        let engine = Self {
            root,
            config,
            disk_resources,
            state: Arc::new(state),
            workspaces: BTreeMap::new(),
            workspace_budget: workspace_budget::WorkspaceBudget::default(),
            blocked: false,
            snapshot: None,
            published_state_base: None,
            history_catalog: None,
            metadata_integrity: Arc::new(provenance::MetadataIntegrity::default()),
            integrity_scrub_schedule: None,
            next_slot: 1,
            journals,
            reads: Arc::new(Mutex::new(BTreeMap::new())),
            upload_preparations: Arc::new(Mutex::new(BTreeSet::new())),
            commit_preparation: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            segment_caches: Arc::new(crate::segment_store::SegmentCaches::new()),
            operation_locator_root: None,
            history_query_proof: Mutex::new(index::QueryProof::default()),
            recovery_warnings: RecoveryWarnings::default(),
            last_publication_stages: None,
            _locks: locks,
            #[cfg(feature = "fault-injection")]
            fault: None,
            #[cfg(feature = "fault-injection")]
            member_faults: Vec::new(),
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
        let disk_resources = Arc::new(DiskResources::open(&config, true)?);
        let locks = acquire_locks(&root, &disk_resources)?;
        let engine = Self {
            root,
            config,
            disk_resources,
            state: Arc::new(State {
                topology_generation: 1,
                ..State::default()
            }),
            workspaces: BTreeMap::new(),
            workspace_budget: workspace_budget::WorkspaceBudget::default(),
            blocked: false,
            snapshot: None,
            published_state_base: None,
            history_catalog: None,
            metadata_integrity: Arc::new(provenance::MetadataIntegrity::default()),
            integrity_scrub_schedule: None,
            next_slot: 1,
            journals: BTreeMap::new(),
            reads: Arc::new(Mutex::new(BTreeMap::new())),
            upload_preparations: Arc::new(Mutex::new(BTreeSet::new())),
            commit_preparation: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            segment_caches: Arc::new(crate::segment_store::SegmentCaches::new()),
            operation_locator_root: None,
            history_query_proof: Mutex::new(index::QueryProof::default()),
            recovery_warnings: RecoveryWarnings::default(),
            last_publication_stages: None,
            _locks: locks,
            #[cfg(feature = "fault-injection")]
            fault: None,
            #[cfg(feature = "fault-injection")]
            member_faults: Vec::new(),
        };
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
        let prepared = engine.prepare_recovery()?;
        engine.seed_workspace_recovery(&prepared)?;
        // Quota admission precedes configuration publication and journal repair.
        write_json(&engine.root.join("config.json"), &engine.config, false)?;
        engine.apply_recovery(RecoveryMode::CompletePending, prepared)?;
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
        Ok(())
    }
    /// Set this engine's retained payload-proof FD cap, not a process FD reservation.
    /// Disk seals, transient I/O and other consumers remain outside this cap.
    pub fn set_max_held_payload_files(&mut self, limit: std::num::NonZeroUsize) {
        self.config.set_max_held_payload_files(limit);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn max_held_payload_files(&self) -> std::num::NonZeroUsize {
        self.config.max_held_payload_files()
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
    /// The unseen event suffix from admitted, publication-ordered State, up to
    /// `limit` events. Cold admission rejects decreasing sequences; writers
    /// only append newer publication slots. The resident tail serves recent
    /// cursors directly; older cursors locate their sealed segment through the
    /// resident directory and read it on demand with a bounded cache.
    pub fn events_page(&self, after: u64, limit: usize) -> Result<Vec<Event>> {
        let members = self.members();
        crate::segment_store::events_page(
            &self.state.events,
            &self.segment_reader(&members),
            after,
            limit,
        )
    }
    /// Total number of published events, for cursor validation and paging.
    pub fn event_count(&self) -> u64 {
        self.state.events.len()
    }
    /// Run one read over published namespace objects: a transaction's fresh
    /// objects never appear here, only hash-verified published pages and
    /// indexes through the bounded cache.
    pub(crate) fn with_namespace_source<T>(
        &self,
        read: impl FnOnce(&dyn crate::namespace::NamespaceSource) -> Result<T>,
    ) -> Result<T> {
        let members = self.members();
        let reader = self.segment_reader(&members);
        let source =
            crate::namespace::LoaderSource(|reference: &storage_contract::NamespaceReference| {
                reader.namespace_node(reference)
            });
        read(&source)
    }
    /// The complete materialized entry list of one commit's namespace in the
    /// legacy canonical order. O(total entries) by definition; point and page
    /// queries go through `with_namespace_source` and the view instead.
    pub(crate) fn namespace_entries(
        &self,
        namespace: &storage_contract::CommitNamespace,
    ) -> Result<Vec<Entry>> {
        self.with_namespace_source(|source| {
            crate::namespace::NamespaceView::new(namespace, source).collect_entries()
        })
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
        self.workspace_view(id).map(std::borrow::Cow::into_owned)
    }
    /// Borrow resident evidence or own a verified, bounded cold-record point read.
    /// The view cannot span an engine mutation. Cold reads can return filesystem
    /// or reconciliation errors; they never populate a permanent manifest cache.
    pub fn workspace_view(&self, id: &str) -> Result<std::borrow::Cow<'_, Workspace>> {
        if let Some(workspace) = self.workspaces.get(id) {
            return Ok(std::borrow::Cow::Borrowed(workspace));
        }
        if let Some(admitted) = self.cold_workspace_retention(id) {
            return self
                .read_cold_workspace(id, admitted)
                .map(std::borrow::Cow::Owned);
        }
        Err(Error::new(
            storage_contract::ErrorCode::WorkspaceNotFound,
            "workspace does not exist",
        ))
    }
    /// Deterministic injection only, never enabled by network or environment input.
    #[cfg(feature = "fault-injection")]
    pub fn inject_fault_once(&mut self, point: &str) {
        self.fault = Some(FaultInjection {
            point: point.to_owned(),
            action: FaultAction::Fail,
        });
    }
    /// Pause a feature-gated failure boundary until the harness observes it and
    /// releases it again. Never configured from requests or environment input.
    #[cfg(feature = "fault-injection")]
    pub fn inject_pause_once(&mut self, point: &str, barrier: Arc<std::sync::Barrier>) {
        self.fault = Some(FaultInjection {
            point: point.to_owned(),
            action: FaultAction::Pause(barrier),
        });
    }
    /// Arm one member-local fault inside a parallel publication stage. Replaces
    /// only a fault for the same (stage, slot); other members can be armed too.
    /// Typed, never parsed from requests or environment.
    #[cfg(feature = "fault-injection")]
    pub fn inject_member_fault_once(
        &mut self,
        stage: StageKind,
        slot: &str,
        fault: MemberFault,
    ) -> Result<()> {
        let disk_id = self.disk(slot)?.disk_id.clone();
        self.member_faults
            .retain(|armed| armed.stage != stage || armed.disk_id != disk_id);
        self.member_faults.push(publication::ArmedMemberFault {
            stage,
            disk_id,
            fault,
        });
        Ok(())
    }
    #[cfg(feature = "fault-injection")]
    pub(crate) fn failpoint(&mut self, point: &str) -> Result<()> {
        if !self
            .fault
            .as_ref()
            .is_some_and(|fault| fault.point == point)
        {
            return Ok(());
        }
        match self
            .fault
            .take()
            .expect("invariant: matching injected fault is present")
            .action
        {
            FaultAction::Fail => Err(Error::new(
                ErrorCode::InjectedFailure,
                format!("interrupted at {point}"),
            )),
            FaultAction::Pause(barrier) => {
                barrier.wait();
                barrier.wait();
                Ok(())
            }
        }
    }
    pub(crate) fn trusted_evidence_members(&self) -> Vec<DiskConfig> {
        self.members()
            .into_iter()
            .filter(|disk| {
                provenance::validate_owned_seal(
                    &self.disk_resources,
                    disk,
                    &self.metadata_integrity,
                )
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
        F: FnOnce(&Self, &mut State, &mut MutationScratch) -> Result<json_codec::Value>,
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
        F: FnOnce(&Self, &mut State, &mut MutationScratch) -> Result<json_codec::Value>,
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
                namespace,
                result,
            } => {
                self.persist_with_objects(*next, prepared, namespace, result.clone())?;
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
        F: FnOnce(&Self, &mut State, &mut MutationScratch) -> Result<json_codec::Value>,
    {
        Self::require_actor(principal)?;
        let digest = digest_override.map(Ok).unwrap_or_else(|| {
            object_hash(
                "request",
                &json!({"kind":kind,"principal":principal,"body":input}),
            )
        })?;
        if let Some(entry) = self.find_receipt(operation_id)? {
            if entry.result.request_digest == digest && entry.principal == principal {
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
