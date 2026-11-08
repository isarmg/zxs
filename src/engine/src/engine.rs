use crate::{disk::*, *};
use json_codec::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

mod backup_preparation;
pub use backup_preparation::{
    BackupCommonDatabasesReport, BackupConfigurationReport, BackupCurrentReadReport,
    BackupDatabaseAlias, BackupDatabaseAliasesReport, BackupDatabaseFile, BackupDatabaseRange,
    BackupDatabaseRead, BackupDatabaseScope, BackupDecidedTargets, BackupDirectoryWriterRoute,
    BackupFileWriterRoute, BackupFirstCandidateReport, BackupGenerationAuthorityReport,
    BackupGenerationControlReport, BackupGenerationPreparationReport, BackupNamespacePage,
    BackupNamespaceScope, BackupOriginalDatabaseReport, BackupPublishedRegistryReport,
    BackupPublishedScopeTargetReport, BackupPublishedScopesReport, BackupRecoveredContent,
    BackupRegistryRecoveryReport, BackupRootDatabasesReport, BackupServingContent,
    BackupServingImagesReport, BackupServingRead, BackupServingTargetReport,
    BackupSystemDatabaseReport, BackupTargetPublicationReport, BackupWriterDestination,
    BackupWriterRoutesReport, open_decided_backup_targets,
};
pub use backup_preparation::{
    BackupCutoverPreparationReport, BackupCutoverPreparationRequest, BackupNativeControlReport,
    BackupNativePreparationReport, BackupNativeTargetReport, BackupObjectDatabasePreparationReport,
    BackupPreparationBatchReport, BackupPreparationPhase, BackupPreparationReport,
    BackupPreparationRequest, BackupPreparationStatus, BackupPreparedTopologyNode,
    BackupScopedDatabasePreparationReport, BackupTopologyTarget,
    offline_check_backup_cutover_inputs, offline_check_backup_native_preparation,
    offline_check_backup_object_databases, offline_check_backup_preparation,
    offline_check_backup_preparation_batch, offline_check_backup_scoped_databases,
};
pub use membership_transition::{
    BackupFirstFinalizeReport, BackupFirstPublicationHash, BackupFirstPublicationReport,
    BackupRegistryDecisionHash,
};
mod config;
pub use config::ValidatedConfig;
mod disk_registry;
pub(crate) use disk_registry::current::{
    CurrentBatchScope, CurrentNamespaces, NamespaceReadView, WorkspaceNamespaceBaseline,
};
pub use disk_registry::{
    DirectoryNamespaceView, DiskRegistrationIdentity, DiskRegistryCommitment,
    HistoricalDiskDefinition, NamespaceEntries, NamespaceSnapshot, NamespaceSnapshotWitness,
    PreparedDiskRegistry, RegisteredDisk, RegistrationMode, RegistryExitEffect,
    RegistryExitRequest, VerifiedDiskRegistry, VerifiedRegistryCandidate,
    VerifiedRegistryPublication,
};
mod disk_lifecycle;
pub(crate) use disk_lifecycle::bootstrap::RecoveredDiskStop;
pub use disk_lifecycle::bootstrap::runtime::InitialActivationProbeGuard;
pub(crate) use disk_lifecycle::terminal::VerifiedTerminalStop;
pub use disk_lifecycle::{
    DeregistrationNamespace, DeregistrationPlanError, DeregistrationPreview,
    FrozenDeregistrationBoundary, ImpactPage, ImpactPageSize, NamespaceDeregistrationPreview,
    NamespaceOccurrenceImpact, NamespaceOccurrenceScan, OccurrenceVisibility, RegisteredHolders,
    RegistrationEffect, TargetPlacementRole, TerminalChainReadmissionPreparation,
    TerminalChainReadmissionProposal, TerminalDiskExit, TerminalManagement,
    TerminalManagementStatus, TerminalRegistrationState, TerminalRetirementArchive,
    TerminalRetirementCheckpoint, TerminalRetirementPhase, TerminalRetirementPlan,
    TerminalRetirementStatus, VersionDeregistrationImpact, VersionImpactScan,
};
pub use disk_registry::published::terminal::TerminalRegistry;
mod planning;
pub(crate) use planning::PlanningContext;
mod disk_catalog;
pub use disk_catalog::{
    DiskCatalogFailure, DiskCatalogMaintenanceState, DiskCatalogObservation, DiskCatalogReceipt,
    DiskCatalogRefreshOutcome, DiskCatalogRefreshReport, DiskCatalogRefreshResult, DiskCatalogRole,
    DiskCatalogSnapshot, DiskCatalogStatus, DiskCatalogVersion, DiskCatalogDirectory,
    DiskDatabaseMaterial,
    DiskDatabaseReadScope, DiskDatabaseRelation, DiskHistoryChange, DiskHistoryFailure,
    DiskHistoryHeader, DiskHistoryMaintenanceState, DiskHistoryObservation, DiskHistoryReceipt,
    DiskHistoryRecord, DiskHistorySnapshot, DiskHistoryStatus, inspect_disk_database_material,
};
mod history_move_recovery;
pub use history_move_recovery::{FileHistoryMoveRecoveryDigest, FileHistoryMoveRecoveryInfo};
mod history_move_decision;
pub use history_move_decision::{
    FileHistoryMoveDecisionDigest, FileHistoryMoveDecisionInputs,
    FileHistoryMoveDecisionObligation, FileHistoryMoveNativePreparation,
};
mod history_move_target;
pub use history_move_target::FileHistoryMoveTargetReport;
mod index;
pub use index::{
    IndexFailure, IndexFailureStage, IndexHealth, SystemAuditEvent, SystemCatalogKind,
    SystemCatalogRecord, SystemCatalogValue, SystemChangeCursor, SystemChangePage,
    SystemChangeReadError, SystemOperationPrincipal, SystemStateMetadata,
};
mod integrity_scrub;
pub use integrity_scrub::{
    IntegrityScrubArea, IntegrityScrubBatch, IntegrityScrubBatchAdmission,
    IntegrityScrubCancellation, IntegrityScrubCompletion, IntegrityScrubIssue,
    IntegrityScrubOutcome, IntegrityScrubPlan, IntegrityScrubReport, IntegrityScrubSchedule,
    IntegrityScrubTarget,
};
mod observations;
mod operations;
pub use observations::{
    DiskObservation, DiskObservationFailure, DiskObservationPlan, DiskObservationStage,
    MetadataMemberProbe, ObservedDiskStatus,
};
pub use operations::{OperationReadObservation, OperationReadPlan, OperationRecord};
pub(crate) mod provenance;
mod publication;
pub(crate) use publication::{DetachedPublication, PublishedDependencyReuse};
mod workspace_commit;
#[cfg(test)]
pub(crate) use workspace_commit::native_workspace_commit_completed;
pub use workspace_commit::{PreparedWorkspaceCommit, WorkspaceCommitPreparation};
mod reads;
pub use reads::{ContentReadPlan, RangeContent, WorkspaceReadPlan};
mod directory_history;
mod directory_layout;
#[path = "engine/history_window/selection.rs"]
mod history_selection;
pub use directory_history::DirectoryHistoryRead;
mod history_window;
pub use history_window::{
    ContentMatchRead, CoupledNodeRetirement, FileHistorySourceRead, HistoryWindowCancellation,
    HistoryWindowRead, PermanentNodeRemovalAnalysis, PermanentNodeRemovalSelection,
    PermanentNodeRetirementOutcome, PermanentRemovalCleanupObservation, RemovalCleanupScope,
    RemovalParentProjection, RemovalReplicaCleanup, RemovalReplicaCleanupState, SharedHistoryRead,
};
pub(crate) mod membership_transition;
pub use membership_transition::{
    DiskExitPhase, DiskExitStatus, DiskReadmissionPhase, DiskReadmissionStatus,
    MembershipAbortStatus, MembershipArchiveInspection, MembershipControlArchive,
    MembershipExitAction, MembershipFenceStatus, MembershipJointDecisionStatus,
    MembershipJointRole, MembershipJointVote, MembershipJournalInspection,
    PendingMemberControlReceipt, PendingMemberDirectory, PendingMembershipStatus,
    verify_membership_control_archive,
};
pub use membership_transition::{
    TerminalChainActivationPlanReport, TerminalChainFirstPublication,
    TerminalChainGenerationLogReport, TerminalChainGenerationSelectionReport,
    TerminalChainIoDecisionReport, TerminalChainPublishedCurrent,
    TerminalChainPublishedCurrentReport, TerminalChainReadmissionActivation,
    TerminalChainReadmissionPhase, TerminalChainReadmissionStatus, TerminalChainSelectedGeneration,
    TerminalMetadataResume,
    TerminalMetadataResumeAdmission, TerminalMetadataResumeCancelOutcome,
    TerminalMetadataResumeCancellationPhase, TerminalMetadataResumePhase,
    TerminalMetadataResumeRecovery, TerminalMetadataResumeStatus, TerminalReadmissionPreview,
};
mod recovery;
use recovery::RecoveryMode;
mod recovery_checkpoint;
mod workspace_budget;
mod workspace_limits;
pub use workspace_limits::RetainedWorkspaceLimits;
#[cfg(feature = "fault-injection")]
mod disk_io_fault;
mod source;
#[cfg(feature = "fault-injection")]
pub use disk_io_fault::DiskQuiesceFault;
mod workspace_storage;
use config::acquire_locks;
pub use source::ReadFile;
pub(crate) use source::ReadSource;
pub use workspace_storage::{WorkspaceInventory, WorkspaceInventoryRecord};

/// Healthy replicas may answer despite rejected members. If they cannot,
/// retain the first concrete rejection rather than claiming evidence is absent.
pub(crate) struct EvidenceMembers {
    pub(crate) members: Vec<DiskConfig>,
    pub(crate) first_error: Option<Error>,
}

/// Explicit nonvoting recovery input with a trusted pre-loss physical binding.
/// Retain this independently of SYSTEM before loss. Never reconstruct its expected
/// identity from a root being recovered: its seal alone cannot authenticate a
/// Genesis data disk's original device/inode. Only `root` may change on relocation.
#[derive(Clone, Debug)]
pub struct RecoveryDataRoot {
    pub root: PathBuf,
    pub identity: DiskRegistrationIdentity,
    pub device: u64,
    pub inode: u64,
}

/// The seal boundary that kept a recovery invocation from admitting evidence.
#[derive(Debug, PartialEq, Eq)]
pub enum RecoverySealContext {
    /// Revalidation on the admitted member's held root.
    Member { disk_id: String, slot_id: String },
    /// The original configuration anchor could not admit this member's journal.
    MembershipAnchor { disk_id: String, slot_id: String },
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
    /// At most one seal/anchor failure per member and one publication barrier.
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
    // Derived lookup only: the published State still proves every historical
    // plan, actor and request binding before a workspace can retire.
    historical_workspace_operations: BTreeMap<String, OperationId>,
    pub(crate) current_namespaces: Option<Arc<CurrentNamespaces>>,
    pub(crate) workspace_namespace_baselines: BTreeMap<String, WorkspaceNamespaceBaseline>,
    workspace_budget: workspace_budget::WorkspaceBudget,
    pub(crate) blocked: bool,
    backup_preparation: Option<backup_preparation::Worker>,
    membership_fence: Option<membership_transition::AdmittedFence>,
    pending_membership: Option<membership_transition::pending::PendingMembership>,
    pending_disk_exit: Option<membership_transition::pending::activation::exit::PendingDiskExit>,
    pub(crate) snapshot: Option<String>,
    published_state_base: Option<Arc<publication::PublishedStateBase>>,
    history_window_roots: Option<Arc<history_window::HistoryRootBindings>>,
    pub(crate) metadata_integrity: Arc<provenance::MetadataIntegrity>,
    integrity_scrub_schedule: Option<std::sync::Weak<integrity_scrub::ScheduleState>>,
    pub(crate) next_slot: u64,
    pub(crate) journals: BTreeMap<String, crate::journal::JournalWriter>,
    pub(crate) reads: Arc<Mutex<ReadRegistry>>,
    pub(crate) upload_preparations: Arc<Mutex<BTreeSet<String>>>,
    commit_preparation: Arc<std::sync::atomic::AtomicBool>,
    history_query_proof: Mutex<index::QueryProof>,
    catalog_maintenance: disk_catalog::Maintenance,
    recovery_warnings: RecoveryWarnings,
    _locks: crate::disk::WriterLocks,
    #[cfg(feature = "fault-injection")]
    fault: Option<FaultInjection>,
}

/// An immutable business-metadata projection, not an authoritative State.
/// This owns no disk resources, read lease, authentication or operation receipts.
/// Callers must recheck current scope; only retained business fields share storage.
struct BusinessMetadataProjection(Arc<State>);

pub struct MetadataView {
    state: BusinessMetadataProjection,
    pub(crate) current_namespaces: NamespaceReadView,
    history_window_roots: Option<Arc<history_window::HistoryRootBindings>>,
    history_action_max_entries: usize,
}

impl MetadataView {
    /// Read-only business fields for listing and history projection only.
    ///
    /// This is not the authoritative State: authentication, operation receipts,
    /// operation principals, events and local plans are deliberately absent.
    /// Never use it for authorization, mutation, persistence or recovery.
    /// Current authorization must be checked against the owning Engine.
    pub fn business_projection(&self) -> &State {
        self.state.0.as_ref()
    }
}

mod metadata_projection;

#[cfg(test)]
#[path = "../../../test/engine/unit/engine/history_window_root_tests.rs"]
mod history_window_root_tests;

#[cfg(test)]
#[path = "../../../test/engine/unit/engine/history_window_retention_tests.rs"]
mod history_window_retention_tests;
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
    reads: Arc<Mutex<ReadRegistry>>,
}
/// A failed lease invariant fences all later admission and destructive maintenance.
#[derive(Default)]
pub(crate) struct ReadRegistry {
    counts: BTreeMap<String, usize>,
    failed: bool,
}
impl ReadRegistry {
    pub(crate) fn counts(&self) -> Result<&BTreeMap<String, usize>> {
        if self.failed {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "read lease registry invariant failed",
            ));
        }
        Ok(&self.counts)
    }
    fn acquire(&mut self, version: String) -> Result<()> {
        self.counts()?;
        *self.counts.entry(version).or_default() += 1;
        Ok(())
    }
}
impl Drop for ReadLease {
    fn drop(&mut self) {
        let mut reads = match self.reads.lock() {
            Ok(reads) => reads,
            Err(_) => {
                // Keep the poisoned registry fenced; its counts cannot be trusted.
                tracing::error!("read lease cleanup blocked by poisoned registry");
                return;
            }
        };
        match reads.counts.get_mut(&self.version) {
            Some(count) if *count > 0 => {
                *count -= 1;
                if *count == 0 {
                    reads.counts.remove(&self.version);
                }
            }
            _ => {
                reads.failed = true;
                tracing::error!("read lease cleanup found an invalid registered count");
            }
        }
    }
}
pub struct OpenedContent {
    pub file: ReadFile,
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
        self.require_unfenced_membership()?;
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
        self.require_unfenced_membership()?;
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
    pub(crate) fn retain_writer_locks(&self) -> Result<crate::disk::WriterLocks> {
        self._locks.retain()
    }

    /// Target-local intent persistence is not a pool publication and does not
    /// admit unrelated disk I/O. The caller holds the target activity lease.
    pub(crate) fn retain_disk_writer_locks(
        &self,
        disk: &DiskConfig,
        root: &crate::disk::DiskRoot,
    ) -> Result<crate::disk::WriterLocks> {
        self._locks.retain_disk(disk, root)
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
        let selection = crate::authority_selection::SelectionGuard::legacy(&root)?;
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
        Self::initialize_resources(root, config, disk_resources, selection)
    }

    /// Initialize exactly the selected, already-open empty storage directories.
    /// The handles are consumed and retained for all writes; their paths are
    /// diagnostic/configuration bindings, never reopened as write destinations.
    /// No storage directory is created, formatted, emptied or chmodded here.
    ///
    /// # Errors
    /// Rejects a changed path binding, non-private or nonempty directory, wrong
    /// handle count, existing pool, or any lock/persistence failure.
    pub fn initialize_held(
        root: &Path,
        config: ValidatedConfig,
        directories: Vec<File>,
    ) -> Result<Self> {
        let root = existing_private_dir(root)?.path().to_path_buf();
        let selection = crate::authority_selection::SelectionGuard::legacy(&root)?;
        if root.join("config.json").try_exists()? {
            return Err(Error::new(
                ErrorCode::AlreadyInitialized,
                "configuration already exists; use open/recovery",
            ));
        }
        if config
            .disks
            .iter()
            .any(|disk| disk.root.starts_with(&root) || root.starts_with(&disk.root))
        {
            return Err(Error::new(
                ErrorCode::ConfigInvalid,
                "storage and system directories must not overlap",
            ));
        }
        let disk_resources = Arc::new(DiskResources::initialize_held(&config, directories)?);
        Self::initialize_resources(root, config, disk_resources, selection)
    }

    fn initialize_resources(
        root: PathBuf,
        config: ValidatedConfig,
        disk_resources: Arc<DiskResources>,
        selection: crate::authority_selection::SelectionGuard,
    ) -> Result<Self> {
        // Hold the same directory-before-writer lock order as online preparation
        // through all initialization writes and index construction. A losing
        // preparation must not install a reservation into this initializer's root.
        let _directory_locks =
            membership_transition::pending::reservation::lock_initialization_directories(
                &disk_resources,
            )?;
        // The same directory lock arbitrates independent-format creation.
        // Admit every root before creating even a legacy writer sidecar, so a
        // different SYSTEM cannot claim an existing independent disk or SYSTEM.
        for disk in disk_resources.handles() {
            if disk
                .root
                .entries(disk.root.path())?
                .next()
                .transpose()?
                .is_some()
            {
                return Err(Error::new(
                    ErrorCode::DiskAlreadyEnrolled,
                    "storage initialization requires empty directories; existing contents are preserved",
                ));
            }
        }
        let locks = config::acquire_locks_selected(&root, &disk_resources, selection)?;
        // Recheck after locking: an earlier empty-root observation can be stale.
        for disk in disk_resources.handles() {
            membership_transition::pending::reservation::reject_reserved_initialization(
                &disk.root,
            )?;
        }
        for d in &config.disks {
            let disk = disk_resources.get(d)?;
            disk.root
                .write_json(&d.root.join("disk.json"), &disk.seal, false)?;
            disk.root.write_atomic(&d.root.join("README.txt"),b"ZXS disk format 2. Ordinary full files are stored below histories/. Use zxs-helper offline-check/offline-export before recovery. Filesystem choice and physical failure-domain independence are the operator responsibility.\n",false)?;
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
        disk_lifecycle::bootstrap::runtime::initialize(&root, &config, &disk_resources)?;
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
            current_namespaces: None,
            workspace_namespace_baselines: BTreeMap::new(),
            historical_workspace_operations: BTreeMap::new(),
            workspace_budget: workspace_budget::WorkspaceBudget::default(),
            blocked: false,
            backup_preparation: None,
            membership_fence: None,
            pending_membership: None,
            pending_disk_exit: None,
            snapshot: None,
            published_state_base: None,
            history_window_roots: None,
            metadata_integrity: Arc::new(provenance::MetadataIntegrity::default()),
            integrity_scrub_schedule: None,
            next_slot: 1,
            journals,
            reads: Arc::new(Mutex::new(ReadRegistry::default())),
            upload_preparations: Arc::new(Mutex::new(BTreeSet::new())),
            commit_preparation: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            history_query_proof: Mutex::new(index::QueryProof::default()),
            catalog_maintenance: disk_catalog::Maintenance::default(),
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

    /// Recover using exactly the pre-admitted storage directory handles. No
    /// writable journal is opened before those handles and their seals have
    /// been validated; path replacement cannot redirect recovery writes.
    pub fn open_for_recovery_held(
        root: &Path,
        workspace_limits: Option<RetainedWorkspaceLimits>,
        directories: Vec<File>,
    ) -> Result<Self> {
        let (root, config) = Self::recovery_projection(root, workspace_limits)?;
        let (config, bootstrap) =
            membership_transition::pending::activation::discover_startup(&root, config)?;
        let resources = Arc::new(DiskResources::recover_held(&config, directories)?);
        let engine = Self::prepare_recovery_resources(root, config, resources)?;
        drop(bootstrap);
        Self::finish_held_recovery(engine)
    }

    fn finish_held_recovery(mut engine: Self) -> Result<Self> {
        engine.admit_disk_exit_io_exclusion()?;
        let warnings = engine.prepare_generation_resources()?;
        let mut prepared = engine.prepare_recovery_with_warnings(warnings)?;
        engine.preflight_workspace_receipts(&mut prepared)?;
        engine.recover_prepared(RecoveryMode::PreservePending, prepared)?;
        if engine.config.generation().is_some() && !engine.generation_activation_published() {
            engine.blocked = true;
        }
        engine.restore_current_disk_exit()?;
        engine.retire_published_readmission_plan()?;
        engine.load_workspaces()?;
        engine.rebuild_index()?;
        Ok(engine)
    }

    fn recovery_projection(
        root: &Path,
        workspace_limits: Option<RetainedWorkspaceLimits>,
    ) -> Result<(PathBuf, ValidatedConfig)> {
        let root = existing_private_dir(root).map_err(|source| source.context(
            ErrorCode::RecoveryRequired,
            "system state is unavailable; use explicit retained metadata roots for management recovery",
        ))?.path().to_path_buf();
        crate::authority_selection::require_legacy(&root)?;
        disk_lifecycle::terminal::require_active_startup(&root)?;
        let config = read_json::<Config>(&root.join("config.json")).map_err(|source| source.context(
            ErrorCode::RecoveryRequired,
            "system projection is unavailable; use explicit retained metadata roots for management recovery",
        ))?;
        let config = ValidatedConfig::try_from(config)?.with_workspace_limits(workspace_limits)?;
        Ok((root, config))
    }

    fn open_mode(
        root: &Path,
        mode: RecoveryMode,
        workspace_limits: Option<RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        let (root, config) = Self::recovery_projection(root, workspace_limits)?;
        let (mut engine, warnings) = Self::prepare_workspace_recovery(root, config)?;
        let mut prepared = engine.prepare_recovery_with_warnings(warnings)?;
        engine.preflight_workspace_receipts(&mut prepared)?;
        engine.recover_prepared(mode, prepared)?;
        if engine.config.generation().is_some() {
            if mode == RecoveryMode::CompletePending {
                engine.complete_generation_activation()?;
            } else if !engine.generation_activation_published() {
                engine.blocked = true;
            }
        }
        engine.restore_current_disk_exit()?;
        engine.retire_published_readmission_plan()?;
        engine.load_workspaces()?;
        engine.rebuild_index()?;
        if mode == RecoveryMode::CompletePending {
            engine.refresh_published_disk_catalogs(None);
            engine.restore_published_disk_histories();
            engine.resume_backup_preparation()?;
        }
        Ok(engine)
    }

    /// Hold the same Engine and writer locks through read-only receipt/workspace
    /// admission, replay and authoritative reconciliation. No journal writer is
    /// opened until both preflights have accepted the retained evidence.
    /// Return fresh admission failures for the following journal diagnostics.
    fn prepare_workspace_recovery(
        root: PathBuf,
        config: ValidatedConfig,
    ) -> Result<(Self, Vec<RecoverySealFailure>)> {
        let (config, bootstrap) =
            membership_transition::pending::activation::discover_startup(&root, config)?;
        let (mut engine, warnings) = Self::prepare_retained_workspace_recovery(root, config)?;
        drop(bootstrap);
        engine.admit_disk_exit_io_exclusion()?;
        Ok((engine, warnings))
    }

    /// Only already authenticated startup probes or explicit retained-root
    /// recovery may enter here. Config's unlisted addresses remain forbidden.
    fn prepare_retained_workspace_recovery(
        root: PathBuf,
        config: ValidatedConfig,
    ) -> Result<(Self, Vec<RecoverySealFailure>)> {
        let disk_resources = Arc::new(DiskResources::open(&config, true)?);
        let mut engine = Self::prepare_recovery_resources(root, config, disk_resources)?;
        let warnings = engine.prepare_generation_resources()?;
        Ok((engine, warnings))
    }

    fn prepare_recovery_resources(
        root: PathBuf,
        config: ValidatedConfig,
        disk_resources: Arc<DiskResources>,
    ) -> Result<Self> {
        let locks = acquire_locks(&root, &disk_resources)?;
        Self::recovery_resources_with_locks(root, config, disk_resources, locks)
    }

    /// Internal ownership transfer from an authenticated management session.
    /// Callers must retain the exact system and disk locks across this boundary.
    fn recovery_resources_with_locks(
        root: PathBuf,
        config: ValidatedConfig,
        disk_resources: Arc<DiskResources>,
        locks: crate::disk::WriterLocks,
    ) -> Result<Self> {
        crate::authority_selection::require_legacy(&root)?;
        let engine = Self {
            root,
            config,
            disk_resources,
            state: Arc::new(State {
                topology_generation: 1,
                ..State::default()
            }),
            workspaces: BTreeMap::new(),
            current_namespaces: None,
            workspace_namespace_baselines: BTreeMap::new(),
            historical_workspace_operations: BTreeMap::new(),
            workspace_budget: workspace_budget::WorkspaceBudget::default(),
            blocked: false,
            backup_preparation: None,
            membership_fence: None,
            pending_membership: None,
            pending_disk_exit: None,
            snapshot: None,
            published_state_base: None,
            history_window_roots: None,
            metadata_integrity: Arc::new(provenance::MetadataIntegrity::default()),
            integrity_scrub_schedule: None,
            next_slot: 1,
            journals: BTreeMap::new(),
            reads: Arc::new(Mutex::new(ReadRegistry::default())),
            upload_preparations: Arc::new(Mutex::new(BTreeSet::new())),
            commit_preparation: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            history_query_proof: Mutex::new(index::QueryProof::default()),
            catalog_maintenance: disk_catalog::Maintenance::default(),
            recovery_warnings: RecoveryWarnings::default(),
            _locks: locks,
            #[cfg(feature = "fault-injection")]
            fault: None,
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
        Self::restore_from_disks_and_data_roots_with_workspace_limits(
            root,
            disks,
            &[],
            workspace_limits,
        )
    }

    /// Capture trusted physical bindings from already admitted data handles.
    /// Keep these records outside SYSTEM for explicit system-loss recovery.
    pub fn recovery_data_roots(&self) -> Result<Vec<RecoveryDataRoot>> {
        self.config
            .io_disks()
            .filter(|disk| !disk.metadata_member)
            .filter_map(|disk| {
                self.disk_resources.available(disk).map(|handle| {
                    let physical = handle.root.directory_identity(handle.root.path())?;
                    let seal = &handle.seal;
                    if physical.device != seal.device || physical.inode != seal.inode {
                        return Err(Error::new(
                            ErrorCode::DiskIdentityMismatch,
                            "held data root differs from its admitted physical identity",
                        ));
                    }
                    Ok(RecoveryDataRoot {
                        root: disk.root.clone(),
                        identity: DiskRegistrationIdentity {
                            pool_id: seal.pool_id.clone(),
                            disk_id: seal.disk_id.clone(),
                            slot_id: seal.slot_id.clone(),
                            binding_generation: seal.binding_generation,
                        },
                        device: seal.device,
                        inode: seal.inode,
                    })
                })
            })
            .collect()
    }

    /// Recover metadata authority first, then admit only explicitly named existing
    /// nonvoting data roots. Data roots never contribute to the metadata quorum.
    /// No configured but unlisted path is probed, initialized or readmitted.
    pub fn restore_from_disks_and_data_roots(
        root: &Path,
        metadata_roots: &[PathBuf],
        data_roots: &[RecoveryDataRoot],
    ) -> Result<Self> {
        Self::restore_from_disks_and_data_roots_with_workspace_limits(
            root,
            metadata_roots,
            data_roots,
            None,
        )
    }

    pub fn restore_from_disks_and_data_roots_with_workspace_limits(
        root: &Path,
        metadata_roots: &[PathBuf],
        data_roots: &[RecoveryDataRoot],
        workspace_limits: Option<RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        let disks = metadata_roots;
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
        let mut anchor_failure = None;
        for path in disks {
            let seal = decode_seal(&read_bytes(&path.join("disk.json"), MAX_METADATA)?)?;
            let path = path.canonicalize()?;
            // Keep every input identity, including rejected anchors. A bad
            // anchor cannot hide a duplicate or a foreign/nonmember disk.
            if seals.insert(seal.disk_id.clone(), path.clone()).is_some() {
                return Err(Error::new(
                    storage_contract::ErrorCode::ConfigConflict,
                    "duplicate disk id in recovery input",
                ));
            }
            let config = match config::read_recovery_anchor(&path, &seal) {
                Ok(config) => config,
                Err(error) => {
                    let code = error.code();
                    anchor_failure.get_or_insert_with(|| {
                        error.context(
                            code,
                            format!(
                                "recovery configuration anchor rejected for disk {} at {}",
                                seal.disk_id,
                                path.display(),
                            ),
                        )
                    });
                    continue;
                }
            };
            let hash = match config.generation_hash() {
                Some(hash) => hash.to_owned(),
                None => object_hash("pool-config", &config)?,
            };
            configs
                .entry(hash)
                .and_modify(|(_, n)| *n += 1)
                .or_insert((config, 1));
        }
        let generation_config = if configs
            .values()
            .any(|(config, _)| config.generation().is_some())
        {
            Some(membership_transition::pending::activation::latest_restore_config(&configs)?)
        } else {
            None
        };
        let mut agreed = configs
            .into_values()
            .filter(|(c, n)| *n >= c.metadata_quorum())
            .map(|(c, _)| c);
        let config = generation_config
            .clone()
            .or_else(|| agreed.next())
            .ok_or_else(|| {
                let message = "a configured metadata majority does not agree on pool configuration";
                match anchor_failure {
                    Some(source) => source.context(ErrorCode::ConfigConflict, message),
                    None => Error::new(ErrorCode::ConfigConflict, message),
                }
            })?;
        if (generation_config.is_none() && agreed.next().is_some())
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
        let forbidden = config
            .generation()
            .map(|generation| generation.portable.forbidden_disk_ids())
            .transpose()?
            .unwrap_or_default();
        // Metadata selection above authenticates the fixed quorum and current
        // lineage before the first syscall against any explicitly named data root.
        let mut data_paths = BTreeMap::new();
        let mut data_seals = BTreeMap::new();
        for expected in data_roots {
            let registered = config
                .disks
                .iter()
                .find(|disk| disk.disk_id == expected.identity.disk_id && !disk.metadata_member)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::ConfigConflict,
                        "explicit recovery identity is not a current nonvoting member",
                    )
                })?;
            if expected.identity.pool_id != config.pool_id
                || expected.identity.slot_id != registered.slot_id
                || expected.identity.binding_generation != registered.binding_generation
                || forbidden.contains(&registered.disk_id)
                || config.excludes_io_disk(&registered.disk_id)
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "explicit recovery cannot change registration or readmit an excluded disk",
                ));
            }
            let held = DiskRoot::open(&expected.root)?;
            let stored =
                decode_seal(&held.read_bytes(&held.path().join("disk.json"), MAX_METADATA)?)?;
            let mut disk = registered.clone();
            disk.root = held.path().to_path_buf();
            let live = identity_at(&config, &disk, &held.directory(held.path())?)?;
            validate_stored_seal(&stored, &live)?;
            if live.disk_id != expected.identity.disk_id
                || live.device != expected.device
                || live.inode != expected.inode
            {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "explicit data root differs from trusted pre-loss physical binding",
                ));
            }
            config.verify_explicit_recovery_data_root(&held, &disk, &live)?;
            data_seals.insert(disk.disk_id.clone(), live);
            if data_paths.insert(disk.disk_id, disk.root).is_some() {
                return Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "duplicate data disk in recovery input",
                ));
            }
        }
        let roots = config
            .disks
            .iter()
            .map(|disk| data_paths.get(&disk.disk_id).unwrap_or(&disk.root).clone())
            .collect();
        let config = config.with_disk_roots(roots)?;
        let allowed = seals
            .keys()
            .chain(data_paths.keys())
            .filter(|id| !forbidden.contains(*id))
            .cloned()
            .collect();
        let config = config
            .with_workspace_limits(workspace_limits)?
            .restrict_io_disks(allowed)?;
        private_dir(root)?;
        let root = existing_private_dir(root)?.path().to_path_buf();
        let (mut engine, warnings) = Self::prepare_retained_workspace_recovery(root, config)?;
        // Resource acquisition must retain the exact identity checked above;
        // disappearance or replacement cannot become a silently unavailable disk.
        for disk in engine
            .config
            .io_disks()
            .filter(|disk| !disk.metadata_member)
        {
            let expected = &data_seals[&disk.disk_id];
            let handle = engine.disk_resources.get(disk)?;
            validate_stored_seal(expected, &handle.seal)?;
            engine
                .config
                .verify_explicit_recovery_data_root(&handle.root, disk, &handle.seal)?;
        }
        let mut prepared = engine.prepare_recovery_with_warnings(warnings)?;
        engine.preflight_workspace_receipts(&mut prepared)?;
        // A quota failure must not publish a locator or system configuration,
        // or replay journal repair. Preflight retains the same admitted roots
        // and writer locks; all exit exclusions still precede writable replay.
        disk_lifecycle::bootstrap::runtime::initialize(
            &engine.root,
            &engine.config,
            &engine.disk_resources,
        )?;
        engine.admit_disk_exit_io_exclusion()?;
        if engine.config.generation().is_none() {
            write_json(&engine.root.join("config.json"), &engine.config, false)?;
        }
        engine.recover_prepared(RecoveryMode::CompletePending, prepared)?;
        if engine.config.generation().is_some() {
            engine.complete_generation_activation()?;
        }
        engine.restore_current_disk_exit()?;
        engine.retire_published_readmission_plan()?;
        engine.load_workspaces()?;
        engine.rebuild_index()?;
        // Restoring SYSTEM has admitted the original authority. Reconstruct its
        // disposable disk projections only now, never elect authority from them.
        // Their individual failure states remain observable on the returned Engine.
        engine.refresh_published_disk_catalogs(None);
        engine.restore_published_disk_histories();
        Ok(engine)
    }
    pub(crate) fn require_recovery_locks(&self) -> Result<()> {
        let admitted_exit = self.current_namespaces.as_ref().is_some_and(|view| {
            self.config
                .generation()
                .and_then(|generation| generation.portable.exit_registry())
                == Some(view.registry())
        }) || self.disk_exit_io_exclusion_admitted()?;
        let required_disks = if admitted_exit {
            // The durable exit proof installed this registry together with the
            // target's closed gate. Historical Config entries cannot demand a
            // lock on that retired binding, or implicitly reopen its root.
            self._locks.require_all(self.config.io_disks())?;
            self.config.io_disks().count()
        } else {
            self._locks.require_all(&self.config.disks)?;
            self.config.disks.len()
        };
        if self._locks.len() != required_disks + 1 {
            return Err(Error::new(
                storage_contract::ErrorCode::RecoveryRequired,
                "recovery requires every admitted disk and its exclusive OS writer lock",
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
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn state(&self) -> &State {
        self.state.as_ref()
    }
    /// Share only business fields, without retaining the authoritative generation.
    /// Field capture is O(number of fields), independent of record counts.
    pub fn metadata_view(&self) -> MetadataView {
        self.view_from_projection(BusinessMetadataProjection::capture(&self.state))
    }
    fn history_window_view(&self) -> MetadataView {
        self.view_from_projection(BusinessMetadataProjection::capture_history_window(
            &self.state,
        ))
    }
    fn view_from_projection(&self, state: BusinessMetadataProjection) -> MetadataView {
        let history_window_roots = self
            .history_window_roots
            .as_ref()
            .and_then(|roots| roots.projected_for(&self.state, &state.0));
        MetadataView {
            state,
            current_namespaces: self.namespace_read_view(),
            history_window_roots,
            history_action_max_entries: self.config.max_entries,
        }
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
    pub(crate) fn trusted_evidence_members(&self) -> EvidenceMembers {
        let mut selected = EvidenceMembers {
            members: Vec::new(),
            first_error: None,
        };
        for disk in self.members() {
            match provenance::validate_owned_seal(
                &self.disk_resources,
                &disk,
                &self.metadata_integrity,
            ) {
                Ok(()) => selected.members.push(disk),
                Err(error) => {
                    let code = error.code();
                    selected.first_error.get_or_insert_with(|| {
                        error.context(
                            code,
                            format!(
                                "evidence member seal unavailable: disk {} slot {}",
                                disk.disk_id, disk.slot_id
                            ),
                        )
                    });
                }
            }
        }
        selected
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
    pub(crate) fn native_request_digest(
        kind: &str,
        principal: &str,
        input: &json_codec::Value,
    ) -> Result<String> {
        Ok(object_hash(
            "request",
            &json!({"kind":kind,"principal":principal,"body":input}),
        )?)
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
        let digest = digest_override
            .map(Ok)
            .unwrap_or_else(|| Self::native_request_digest(kind, principal, &input))?;
        if let Some(result) = self.state.operations.get(&operation_id) {
            if result.request_digest == digest
                && self.operation_owner(operation_id)?.as_deref() == Some(principal)
            {
                return Ok(MutationPlan::Existing(result.clone()));
            }
            return Err(Error::new(
                storage_contract::ErrorCode::IdempotencyKeyReused,
                "operation id is already bound to different input",
            ));
        }
        self.require_write()?;
        let context = self.planning_context();
        if kind == "smb.binding.update" {
            return context.plan_smb_binding_mutation(
                principal,
                operation_id,
                digest,
                |_, next, prepared| change(self, next, prepared),
            );
        }
        if kind == "plan.cancel" {
            return context.plan_cancellation_mutation(
                principal,
                operation_id,
                digest,
                |_, next, prepared| change(self, next, prepared),
            );
        }
        if kind == "branch.archive" {
            return context.plan_branch_archive_mutation(
                principal,
                operation_id,
                digest,
                |_, next, prepared| change(self, next, prepared),
            );
        }
        if kind == "backup.configure"
            || crate::protection::RecursiveBackupMode::from_kind(kind).is_some()
        {
            return context.plan_backup_configuration_mutation(
                kind,
                principal,
                operation_id,
                digest,
                |_, next, prepared| change(self, next, prepared),
            );
        }
        if kind == "merge-link.create" {
            return context.plan_merge_link_mutation(
                principal,
                operation_id,
                digest,
                |_, next, prepared| change(self, next, prepared),
            );
        }
        if kind == "file.tags.update" {
            return context.plan_file_tags_mutation(
                principal,
                operation_id,
                digest,
                |_, next, prepared| change(self, next, prepared),
            );
        }
        if matches!(
            kind,
            "directory.changes.commit"
                | "directory.git.initialize"
                | "directory.git.stage-child"
                | "directory.git.stage-owned"
                | "directory.git.branch-create"
                | "directory.git.checkout"
        ) {
            return context.plan_directory_commit_mutation(
                kind,
                principal,
                operation_id,
                digest,
                |_, next, prepared| change(self, next, prepared),
            );
        }
        context.plan_new_mutation(
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
