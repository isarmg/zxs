//! Durable background preparation of complete history material. It never changes
//! active Config, Current, routes or old receipts. Explicit later native choice
//! stages still do not claim a serving target or release the source fence.
use super::Engine;
use crate::{disk::*, history_package::PreparedHistoryPackage, *};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::linux::AdvisoryLock;
use std::{
    collections::BTreeMap,
    fs::File,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

mod batch;
mod completion;
mod execution;
mod inspection;
mod native_control;
pub use native_control::{
    BackupCommonDatabasesReport, BackupConfigurationReport, BackupCurrentReadReport,
    BackupDatabaseAlias, BackupDatabaseAliasesReport, BackupDatabaseFile, BackupDatabaseRange,
    BackupDatabaseRead, BackupDatabaseScope, BackupDecidedTargets, BackupDirectoryWriterRoute,
    BackupFileWriterRoute, BackupFirstCandidateReport, BackupGenerationAuthorityReport,
    BackupGenerationControlReport, BackupGenerationPreparationReport, BackupNamespacePage,
    BackupNamespaceScope, BackupNativeMetadataReport, BackupOrdinaryServingReport,
    BackupOrdinaryServingTargetReport, BackupOriginalControlMemberReport,
    BackupOriginalControlReport, BackupOriginalDatabaseReport, BackupOriginalJournalReport,
    BackupPublishedRegistryReport, BackupPublishedScopeTargetReport, BackupPublishedScopesReport,
    BackupRecoveredContent, BackupRegistryRecoveryReport, BackupRootDatabasesReport,
    BackupServingContent, BackupServingImagesReport, BackupServingRead, BackupServingTargetReport,
    BackupSystemDatabaseReport, BackupTargetLocatorReport, BackupTargetPublicationReport,
    BackupTargetSealReport, BackupTargetSealsReport, BackupTargetWriterIoReport,
    BackupTargetWriterReport, BackupTargetWritersReport, BackupWriterDestination,
    BackupWriterIoReport, BackupWriterRoutesReport, open_decided_backup_targets,
};
pub use native_control::{
    BackupCutoverPreparationReport, BackupCutoverPreparationRequest, BackupNativeControlReport,
    BackupNativePreparationReport, BackupNativeTargetReport, BackupObjectDatabasePreparationReport,
    BackupPreparedTopologyNode, BackupScopedDatabasePreparationReport, BackupTopologyTarget,
    offline_check_backup_cutover_inputs, offline_check_backup_native_preparation,
    offline_check_backup_object_databases, offline_check_backup_scoped_databases,
};
pub(in crate::engine) use native_control::{BackupNativeObjects, ServingSwitch};
pub(in crate::engine) use native_control::{
    BackupRegistryProjection, BackupSourceFenceEvidence, MAX_DECISION_BYTES, PreparedBackupRegistry,
};
mod snapshot;
mod target_resources;
mod transaction;
pub use batch::{BackupPreparationBatchReport, offline_check_backup_preparation_batch};
pub use inspection::{BackupPreparationReport, offline_check_backup_preparation};
pub use transaction::{
    BackupTransactionCancelOutcome, BackupTransactionCancellation, BackupTransactionRequest,
};

const FORMAT: &str = "zxs-backup-history-preparation";
const JOB: &str = "backup-preparation/job.json";
const PROGRESS: &str = "backup-preparation/progress.json";
const TARGET_INTENT: &str = "backup-preparation.json";
const RESERVE: u64 = 16 * 1024 * 1024;

/// Produced only by the final same-operation coordinator after actual ordinary
/// entities, original object databases, selectors, seals and writer ownership
/// have crossed the common durable barrier. No stored JSON deserializes to it.
/// Keep construction inside backup_preparation, not ordinary namespace code.
#[derive(Clone)]
pub(in crate::engine) struct ServingBarrier {
    operation: OperationId,
    authority: String,
    candidate: String,
    publication: String,
    registry: DiskRegistryCommitment,
    config_hash: String,
    first_sequence: u64,
    first_snapshot: String,
    system_database_hash: String,
    replayed_current: bool,
}
impl ServingBarrier {
    pub(in crate::engine) fn require_native(
        &self,
        first: &super::membership_transition::PreparedBackupFirstCandidate,
        publication: &super::membership_transition::PreparedBackupFirstPublication,
        registry: &DiskRegistryCommitment,
    ) -> Result<()> {
        publication.require_namespace_candidate(first)?;
        if self.operation != first.candidate().operation_id
            || self.authority != first.authority_hash()
            || self.candidate != first.hash()
            || self.publication != publication.report().publication_hash
            || &self.registry != registry
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "Backup serving barrier differs from its actual original native Publication",
            ));
        }
        Ok(())
    }
    pub(in crate::engine) fn require_config(&self, config: &ValidatedConfig) -> Result<()> {
        if config.generation_hash() != Some(self.authority.as_str())
            || object_hash("backup-effective-config", config)? != self.config_hash
            || config
                .generation()
                .and_then(|generation| generation.registry())
                != Some(&self.registry)
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "Backup serving resources differ from their completed native configuration",
            ));
        }
        Ok(())
    }
    pub(in crate::engine) fn require_current(&self, engine: &Engine) -> Result<()> {
        self.require_config(&engine.config)?;
        if engine.state.sequence < self.first_sequence
            || (engine.state.sequence == self.first_sequence
                && engine.snapshot.as_deref() != Some(self.first_snapshot.as_str()))
            || engine.current_namespaces.as_ref().is_none_or(|view| {
                view.registry() != &self.registry
                    || view.activation_sequence() != self.first_sequence
            })
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "Backup serving barrier has no exact replayed active Current",
            ));
        }
        Ok(())
    }
    pub(in crate::engine) fn authority(&self) -> &str {
        &self.authority
    }
    pub(in crate::engine) fn first_sequence(&self) -> u64 {
        self.first_sequence
    }
    pub(in crate::engine) fn system_database_hash(&self) -> &str {
        &self.system_database_hash
    }
    pub(in crate::engine) fn uses_replayed_current(&self) -> bool {
        self.replayed_current
    }
}

/// Native cold admission caches the pure first view, not a late raw snapshot.
/// Every later slot still advances it through the ordinary exact-write verifier.
pub(super) struct ServingAdmission {
    barrier: ServingBarrier,
    first: Arc<crate::engine::disk_registry::current::CurrentNamespaces>,
    result: OperationResult,
    status: BackupPreparationStatus,
}
impl ServingAdmission {
    fn capture(
        barrier: ServingBarrier,
        current: Arc<crate::engine::disk_registry::current::CurrentNamespaces>,
        first: &super::membership_transition::PreparedBackupFirstCandidate,
        target_count: usize,
    ) -> Result<Self> {
        let files = (first.available_versions() as u64)
            .checked_mul(target_count as u64)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    "Backup serving file count overflow",
                )
            })?;
        let bytes = first
            .candidate()
            .versions
            .iter()
            .try_fold(0u64, |sum, version| {
                sum.checked_add(version.version.logical_size)
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::ResourceLimit,
                            "Backup serving byte count overflow",
                        )
                    })
            })?
            .checked_mul(target_count as u64)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    "Backup serving copy byte count overflow",
                )
            })?;
        let status = BackupPreparationStatus {
            operation_id: first.result().operation_id, phase: BackupPreparationPhase::GenerationServingActive,
            completed_files: files, total_files: files, completed_bytes: bytes, total_bytes: bytes,
            error_code: None, detail: "original Backup operation installed actual Current, all retained writers and SYSTEM; original data remains retained".into(),
        };
        Ok(Self {
            barrier,
            first: current,
            result: first.result().clone(),
            status,
        })
    }
}
impl Engine {
    pub(in crate::engine) fn is_backup_first_candidate(
        &self,
        candidate: &crate::journal::Candidate,
    ) -> Result<bool> {
        let Some(generation) = self.config.generation() else {
            return Ok(false);
        };
        Ok(generation.portable.is_backup() && candidate.sequence == generation.first_sequence()?)
    }
    pub(in crate::engine) fn preflight_backup_first_current(
        &self,
        candidate: &crate::journal::Candidate,
        published: &crate::history_package::AdmittedState,
        publication: &crate::journal::Publication,
        seal: &crate::journal::PublicationSeal,
    ) -> Result<crate::engine::disk_registry::current::CurrentNamespaces> {
        let admission = self.backup_serving.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "Backup Current lacks completed all-root native admission",
            )
        })?;
        let barrier = &admission.barrier;
        crate::journal::verify_publication_seal(candidate, publication, seal)?;
        if candidate.sequence != barrier.first_sequence
            || candidate.snapshot_hash != barrier.first_snapshot
            || candidate.operation_id != barrier.operation
            || object_hash("candidate", candidate)? != barrier.candidate
            || object_hash("publication", publication)? != barrier.publication
            || publication.result != admission.result
            || published.state().sequence != barrier.first_sequence
            || !admission.first.matches_source(published.state())
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "Backup first Current differs from its actual native cold publication barrier",
            ));
        }
        Ok((*admission.first).clone())
    }
    pub(in crate::engine) fn restore_backup_current(&mut self) -> Result<()> {
        let admission = self.backup_serving.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "Backup recovery has no all-root completed serving admission",
            )
        })?;
        let barrier = &admission.barrier;
        if self.state.sequence < barrier.first_sequence {
            return Ok(());
        }
        if let Some(view) = &self.current_namespaces {
            if view.registry() != &barrier.registry
                || view.activation_sequence() != barrier.first_sequence
            {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "Backup Current changed its native first registry",
                ));
            }
            return Ok(());
        }
        if self.state.sequence != barrier.first_sequence
            || self.snapshot.as_deref() != Some(barrier.first_snapshot.as_str())
            || !admission.first.matches_source(&self.state)
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "Backup Current must replay its exact first publication before any later snapshot",
            ));
        }
        self.workspace_namespace_baselines.clear();
        self.current_namespaces = Some(Arc::clone(&admission.first));
        Ok(())
    }
}

/// Explicit complete-copy selection, never an inferred replica policy. The
/// single-target API accepts one path; the batch API accepts two to eight. Both
/// retain the entire supported single-primary pool, never per-file shards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BackupPreparationRequest {
    pub operation_id: OperationId,
    pub principal: String,
    pub source_slot_id: String,
    pub target_paths: Vec<PathBuf>,
    #[serialization(with = "storage_contract::u64_string")]
    pub expected_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum BackupPreparationPhase {
    Copying,
    Verifying,
    /// Frozen history/State material only. Native control closure, database projections and
    /// the topology/placement activation decision are not implemented here.
    HistoryPrepared,
    /// Complete original fixed-genesis native evidence only. No target
    /// membership, database installation, or final authority switch.
    NativeControlPrepared,
    /// All selected targets retain file/directory SQLite from complete State.
    /// SYSTEM/disk historical scopes and authoritative cutover remain absent.
    ObjectDatabasesPrepared,
    /// SYSTEM-current and original disk-current/history SQLite only. Full
    /// historical scopes, replacement bindings and cutover remain absent.
    ScopedDatabasesPrepared,
    /// Explicit complete prospective subtree accepted in the preparation domain.
    /// This material stage alone installs no source fence, final authority or
    /// active fanout route; explicit native fencing is a separate operation.
    CutoverInputsPrepared,
    /// Independent entity images on every selected target, still isolated. The
    /// source remains fenced; no target membership or final placement exists.
    TargetEntitiesPrepared,
    /// Exact registry and whole-unit placements selected in every original
    /// native decision slot. No active writer/route or source fence release.
    RegistryPlacementDecided,
    /// Exact complete fanout and whole-unit writer-route material exists on
    /// every selected target. No native generation publication or writer lease.
    WriterRoutesPrepared,
    /// Complete unpublished native routing snapshots on every selected target.
    /// No final Candidate, active topology, writer lease or source release.
    GenerationInputsPrepared,
    /// Normal native predecessor archives including actual original fenced
    /// journals on every target. No successor authority or writer activation.
    GenerationControlPrepared,
    /// Strict native Backup authority object on every isolated target. It is
    /// not a first publication or an active Config/Current/writer installation.
    GenerationAuthorityPrepared,
    /// Exact first native Candidate and complete prospective State, read back
    /// on all isolated targets. No ACCEPT/FINALIZE/Publication or writer grant.
    GenerationFirstCandidatePrepared,
    /// Real ACCEPT and FINALIZE in every original member's reserved first
    /// generation journal. No Publication, active target writer or source release.
    GenerationFirstCandidateFinalized,
    /// Actual first Publication, all-original seal and route-local proofs in
    /// the reserved generation. Still no active Config/writer or source release.
    GenerationFirstCandidatePublished,
    /// Exact real first Publication and seal, with complete payload proof
    /// readback, retained on every selected target (including empty histories).
    /// Still no active Config/Current, target writer or source fence release.
    GenerationFirstPublicationRetained,
    /// Actual selected Config and exact native registry graph reconstructed
    /// after full Publication admission. No serving Current or target I/O.
    GenerationConfigurationResolved,
    /// Real native parent/successor registry pages and exact selected Config
    /// retained on all targets after actual Publication. Frozen namespace reads
    /// only: no ordinary Current/writer installation or source fence release.
    GenerationRegistryInstalled,
    /// Complete independently copied published entities, object SQLite and
    /// locators on all selected targets. Still no ordinary Current/writer,
    /// final SYSTEM/disk scope installation or source fence release.
    GenerationServingImagesInstalled,
    /// Final published scoped SQLite retained on all targets, with exact own
    /// root copies. Active SYSTEM/Current/writers and source release are absent.
    GenerationPublishedScopesInstalled,
    /// Common published database copies on SYSTEM/all original disks/targets.
    /// Active aliases, Current/writers and the source fence remain unchanged.
    GenerationCommonDatabasesInstalled,
    /// Every target's exact first published namespace has read-only Current
    /// admission after all common databases. This cannot install an active
    /// CurrentNamespaces, a writer or a source-fence release.
    GenerationCurrentReadAdmitted,
    /// The same published database set resolves by typed identity on all
    /// participants. Normal active aliases/writer and source fence are unchanged.
    GenerationDatabaseAliasesInstalled,
    /// Ordinary current/history names on every original disk now hold the
    /// published pair, with displaced files retained. The source pool stays
    /// stopped; SYSTEM, active Current and target writers are not switched.
    GenerationOrdinaryRootDatabasesInstalled,
    /// The ordinary SYSTEM index now holds its published bytes after all old
    /// SQLite readers/writer closed and old main/sidecars were retained. Query
    /// admission stays suspended; no active Current/writer or source release.
    GenerationOrdinarySystemDatabaseInstalled,
    /// Exact native objects exist at all ordinary disk addresses and target
    /// Current replays those bytes. No active Config/locator/writer or unfence.
    GenerationOrdinaryNativeMetadataInstalled,
    /// Every selected target has its actual ordinary writer lock held by this
    /// Engine task. This is fencing, not active DiskWriter/Current or unfence.
    GenerationTargetWriterFencesInstalled,
    /// Exact target-only ordinary DiskWriter/gates are retained by the task.
    /// Not active Engine Current, ordinary pool writes or source fence release.
    GenerationTargetWriterIoPrepared,
    /// Actual native disk.json files are installed and inode-bound while the
    /// exact original task still owns their writers. No generation selector,
    /// active Engine Current or safe source-fence release is implied.
    GenerationTargetSealsInstalled,
    /// All target-only ordinary native locators/archives have independently
    /// replayed under the retained writers. The active pool remains fenced.
    GenerationTargetLocatorsInstalled,
    /// Original roots retain independent native control beside exact first journals.
    /// No active source selector or Engine Current is selected.
    GenerationOriginalControlInstalled,
    /// Complete native-reader target containers/SQLite/locators, still task-local.
    /// No active Current, source selector, next Publication or fence release.
    GenerationOrdinaryServingInstalled,
    /// Actual native Current, retained writers and SYSTEM are serving.
    GenerationServingActive,
    Blocked,
    SourceChanged,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BackupPreparationStatus {
    pub operation_id: OperationId,
    pub phase: BackupPreparationPhase,
    pub completed_files: u64,
    pub total_files: u64,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub error_code: Option<ErrorCode>,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct PhysicalBinding {
    device: u64,
    inode: u64,
}
impl PhysicalBinding {
    fn of(root: &DiskRoot) -> Result<Self> {
        let identity = root.directory_identity(root.path())?;
        Ok(Self {
            device: identity.device,
            inode: identity.inode,
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Job {
    format: String,
    request: BackupPreparationRequest,
    pool_id: String,
    snapshot: String,
    configuration_hash: String,
    target: PhysicalBinding,
    history_roots: BTreeMap<String, String>,
    total_files: u64,
    total_bytes: u64,
}
impl Job {
    fn status(&self) -> BackupPreparationStatus {
        BackupPreparationStatus {
            operation_id: self.request.operation_id,
            phase: BackupPreparationPhase::Copying,
            completed_files: 0,
            total_files: self.total_files,
            completed_bytes: 0,
            total_bytes: self.total_bytes,
            error_code: None,
            detail: "preparing complete history material; active placement unchanged".into(),
        }
    }
    fn binding_matches(&self, engine: &Engine) -> Result<bool> {
        Ok(self.pool_id == engine.config.pool_id
            && self.request.expected_sequence == engine.state.sequence
            && engine.snapshot.as_deref() == Some(self.snapshot.as_str())
            && self.configuration_hash == object_hash("backup-preparation-config", &engine.config)?)
    }
}

pub(super) struct Worker {
    job: Job,
    batch: Option<batch::Batch>,
    state: Arc<Mutex<BackupPreparationStatus>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    // Retained across task completion, errors, inspection and exact retries.
    targets: Vec<target_resources::Target>,
}
impl Worker {
    fn idle(job: Job, error: Error, phase: BackupPreparationPhase) -> Self {
        let mut status = job.status();
        status.phase = phase;
        status.error_code = Some(error.code());
        status.detail = error.to_string();
        Self {
            job,
            batch: None,
            state: Arc::new(Mutex::new(status)),
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            targets: Vec::new(),
        }
    }
    fn unavailable_status(&self, detail: &str) -> BackupPreparationStatus {
        let mut status = self
            .batch
            .as_ref()
            .map_or_else(|| self.job.status(), batch::Batch::status);
        status.phase = BackupPreparationPhase::Blocked;
        status.error_code = Some(ErrorCode::RecoveryRequired);
        status.detail = detail.into();
        status
    }
    fn status(&self) -> BackupPreparationStatus {
        // Observe completion before reading the cache: doing this in the reverse
        // order could mistake a just-completed healthy update for a lost result.
        let finished = self.thread.as_ref().is_some_and(JoinHandle::is_finished);
        match self.state.lock() {
            Ok(status)
                if finished
                    && matches!(
                        status.phase,
                        BackupPreparationPhase::Copying | BackupPreparationPhase::Verifying
                    ) =>
            {
                self.unavailable_status(
                    "backup worker exited without terminal progress; durable task retained",
                )
            }
            Ok(status) => status.clone(),
            // This is only a diagnostic cache, never authoritative business
            // state. A poisoned value is discarded, not treated as healthy.
            Err(_) => self.unavailable_status(
                "backup progress cache is poisoned; durable task and original data retained",
            ),
        }
    }
    fn join(&mut self) -> Result<()> {
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            let detail =
                "backup preparation worker panicked; durable task and original data retained";
            let failed = self.unavailable_status(detail);
            match self.state.lock() {
                Ok(mut status) => *status = failed,
                // Replace the untrusted diagnostic with a closed failure. Do
                // not clear poisoning or recover any business data from it.
                Err(poisoned) => *poisoned.into_inner() = failed,
            }
            return Err(Error::new(ErrorCode::RecoveryRequired, detail));
        }
        Ok(())
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // No Engine lock is needed by this worker. Copy/checksum readers observe
        // bounded checkpoints; an OS I/O call can still delay shutdown.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Engine {
    /// Persist a single whole-pool history preparation and automatically start
    /// its worker. `directory` is the exact selected empty private target root.
    /// No disk is enrolled and no replacement is reported as complete.
    pub fn start_backup_preparation(
        &mut self,
        request: BackupPreparationRequest,
        directory: File,
    ) -> Result<BackupPreparationStatus> {
        Self::require_actor(&request.principal)?;
        self.require_write()?;
        self.require_unfenced_membership()?;
        self.check_backup_preparation_scope(&request)?;
        let system = DiskRoot::open(&self.root)?;
        transaction::require_operation_open(&system, request.operation_id)?;
        batch::require_no_batch(&system)?;
        if crate::history_package::preparation_path_exists(&system, &self.root.join(JOB))? {
            let job: Job = decode_strict(&system.read_bytes(&self.root.join(JOB), MAX_METADATA)?)?;
            if job.request != request {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "a different backup preparation is already retained; no target is replaced",
                ));
            }
            let target = DiskRoot::from_directory(request.target_paths[0].clone(), directory)?;
            if PhysicalBinding::of(&target)? != job.target {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "backup target binding changed",
                ));
            }
            let running = self
                .backup_preparation
                .as_ref()
                .and_then(|worker| worker.thread.as_ref())
                .is_some_and(|thread| !thread.is_finished());
            if !running {
                if let Some(previous) = self.backup_preparation.as_mut() {
                    previous.join()?;
                }
                self.resume_backup_preparation()?;
            }
            return self.backup_preparation_status().ok_or_else(|| {
                Error::new(ErrorCode::RecoveryRequired, "persisted task did not resume")
            });
        }
        let target = DiskRoot::from_directory(request.target_paths[0].clone(), directory)?;
        reject_overlap(self, &target)?;
        let lock = AdvisoryLock::directory(&target.directory(target.path())?).map_err(fs_error)?;
        let plans = self.backup_history_plans()?;
        let (history_roots, total_files, total_bytes) = describe(&plans)?;
        let job = Job {
            format: FORMAT.into(),
            request,
            pool_id: self.config.pool_id.clone(),
            snapshot: self.snapshot.clone().ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "preparation requires a published snapshot",
                )
            })?,
            configuration_hash: object_hash("backup-preparation-config", &self.config)?,
            target: PhysicalBinding::of(&target)?,
            history_roots,
            total_files,
            total_bytes,
        };
        // The system intent is durable before the first target write. Startup
        // can finish the empty-target reservation after this crash boundary.
        if target.entries(target.path())?.next().transpose()?.is_some() {
            return Err(Error::new(
                ErrorCode::ObjectCollision,
                "backup preparation requires an empty target; existing bytes preserved",
            ));
        }
        let snapshot = snapshot::PreparedSnapshot::freeze(self, &job)?;
        let intent = canonical_json(&job)?;
        if intent.len() > MAX_METADATA {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "backup task exceeds metadata budget",
            ));
        }
        system.private_dir(&self.root.join("backup-preparation"))?;
        system.write_atomic(&self.root.join(JOB), &intent, false)?;
        reserve_target(&target, &job)?;
        self.backup_preparation = Some(spawn(self, job, system, target, lock, plans, snapshot)?);
        Ok(self
            .backup_preparation_status()
            .expect("invariant: worker was installed"))
    }

    /// Runtime observation. A later source publication cannot silently become
    /// the old task's new baseline or be described as fully prepared.
    pub fn backup_preparation_status(&self) -> Option<BackupPreparationStatus> {
        if let Some(admitted) = &self.backup_serving {
            let mut status = admitted.status.clone();
            let ready = admitted.barrier.require_current(self).and_then(|()| {
                if self.state.operations.get(&admitted.barrier.operation) != Some(&admitted.result)
                    || !matches!(self.index_health(), super::index::IndexHealth::Ready { sequence } if sequence == self.state.sequence)
                    || self.backup_cutover_fence_status().is_some_and(|fence| {
                        fence.operation_id == admitted.barrier.operation
                            && fence.transition_hash == admitted.result.request_digest
                    })
                {
                    return Err(Error::new(ErrorCode::RecoveryRequired,
                        "Backup native switch is retained, but receipt, SYSTEM or original write-stop completion is not ready"));
                }
                Ok(())
            });
            if let Err(error) = ready {
                status.phase = BackupPreparationPhase::Blocked;
                status.error_code = Some(error.code());
                status.detail = error.to_string();
            }
            return Some(status);
        }
        let worker = self.backup_preparation.as_ref()?;
        let mut status = worker.status();
        match worker.job.binding_matches(self) {
            Ok(true) => {}
            Ok(false) => {
                status.phase = BackupPreparationPhase::SourceChanged;
                status.error_code = Some(ErrorCode::ConfigConflict);
                status.detail = "source publication changed; frozen history remains evidence, not a current replacement".into();
            }
            Err(error) => {
                status.phase = BackupPreparationPhase::Blocked;
                status.error_code = Some(error.code());
                status.detail =
                    format!("cannot verify the frozen preparation source boundary: {error}");
            }
        }
        if let Some(fence) = self.backup_cutover_fence_status() {
            status.error_code.get_or_insert(ErrorCode::RecoveryRequired);
            // Preserve the worker's actual phase/error. Source authority is a
            // separate fact and must not erase in-progress/capacity diagnostics.
            status.detail = format!(
                "original generation remains write-fenced ({}/{} native receipts); no final target authority: {}",
                fence.confirmed_members, fence.original_members, status.detail,
            );
        }
        Some(status)
    }

    /// Host shutdown/test synchronization; ordinary setting calls do not wait.
    pub fn wait_backup_preparation(&mut self) -> Result<BackupPreparationStatus> {
        self.backup_preparation
            .as_mut()
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "no backup preparation"))?
            .join()?;
        Ok(self
            .backup_preparation_status()
            .expect("invariant: retained worker"))
    }

    pub(super) fn resume_backup_preparation(&mut self) -> Result<()> {
        if self
            .config
            .generation()
            .is_some_and(|generation| generation.portable.is_backup())
        {
            // Active native Backup has its own completed/partial cold boundary;
            // old preparation files cannot restart a copier in the successor.
            return Ok(());
        }
        let system = DiskRoot::open(&self.root)?;
        if transaction::cancellation_pending(&system)? {
            // A durable explicit cancel cannot be turned back into copy work by
            // cold open. Only the same-operation cancellation may finish archiving.
            return Ok(());
        }
        if native_control::resume_fenced(self, &system)? {
            return Ok(());
        }
        if native_control::resume(self, &system)? {
            return Ok(());
        }
        if batch::resume(self, &system)? {
            return Ok(());
        }
        if !crate::history_package::preparation_path_exists(&system, &self.root.join(JOB))? {
            return Ok(());
        }
        let job: Job = decode_strict(&system.read_bytes(&self.root.join(JOB), MAX_METADATA)?)?;
        if job.format != FORMAT || job.request.target_paths.len() != 1 {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "unsupported retained backup task; original data preserved",
            ));
        }
        if !job.binding_matches(self)? {
            self.backup_preparation = Some(Worker::idle(
                job,
                Error::new(
                    ErrorCode::ConfigConflict,
                    "source publication changed; automatic preparation does not resample it",
                ),
                BackupPreparationPhase::SourceChanged,
            ));
            return Ok(());
        }
        let resumed = (|| {
            self.require_write()?;
            self.require_unfenced_membership()?;
            self.check_backup_preparation_scope(&job.request)?;
            let target = DiskRoot::open(&job.request.target_paths[0])?;
            reject_overlap(self, &target)?;
            if PhysicalBinding::of(&target)? != job.target {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "retained backup target binding changed",
                ));
            }
            let lock =
                AdvisoryLock::directory(&target.directory(target.path())?).map_err(fs_error)?;
            let plans = self.backup_history_plans()?;
            if describe(&plans)? != (job.history_roots.clone(), job.total_files, job.total_bytes) {
                return Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "prepared history closure differs from the persisted task",
                ));
            }
            let snapshot = snapshot::PreparedSnapshot::freeze(self, &job)?;
            reserve_target(&target, &job)?;
            spawn(self, job.clone(), system, target, lock, plans, snapshot)
        })();
        self.backup_preparation = Some(match resumed {
            Ok(worker) => worker,
            Err(error) => Worker::idle(job, error, BackupPreparationPhase::Blocked),
        });
        Ok(())
    }

    fn check_backup_preparation_scope(&self, request: &BackupPreparationRequest) -> Result<()> {
        if request.target_paths.len() != 1
            || !request.target_paths[0].is_absolute()
            || self
                .registered_disks()
                .filter(|disk| disk.level == 1)
                .count()
                != 1
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "history preparation currently requires one explicitly selected target and one primary source; no implicit sharding or fanout",
            ));
        }
        if request.expected_sequence != self.state.sequence {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "source sequence changed",
            ));
        }
        let source = self.disk(&request.source_slot_id)?;
        if self
            .registered_disk(&source.disk_id)
            .is_none_or(|disk| disk.registration_mode != super::RegistrationMode::Enabled)
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "source is no longer registered",
            ));
        }
        let targets = self
            .config
            .disks
            .iter()
            .filter(|disk| {
                self.registered_disk(&disk.disk_id).is_some()
                    && disk.upstream_slot_id.as_deref() == Some(source.slot_id.as_str())
            })
            .collect::<Vec<_>>();
        if targets.len() != 1
            || self.config.disks.iter().any(|disk| {
                self.registered_disk(&disk.disk_id).is_some()
                    && disk.upstream_slot_id.as_deref() == Some(targets[0].slot_id.as_str())
            })
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "only an existing terminal backup edge is supported for preparation",
            ));
        }
        if self.workspaces.values().any(|workspace| {
            matches!(
                workspace.state,
                WorkspaceState::Open | WorkspaceState::Sealed
            )
        }) || !self
            .upload_preparations
            .lock()
            .expect("invariant: upload preparation lock")
            .is_empty()
        {
            return Err(Error::new(
                ErrorCode::WorkspaceBusy,
                "retained or preparing workspaces require a separate complete transfer protocol",
            ));
        }
        let primary = self
            .config
            .disks
            .iter()
            .find(|disk| disk.level == 1 && self.registered_disk(&disk.disk_id).is_some())
            .expect("invariant: exactly one primary");
        if self
            .registered_disk(&targets[0].disk_id)
            .is_none_or(|disk| disk.registration_mode != super::RegistrationMode::Enabled)
            || self
                .state
                .placements
                .values()
                .any(|placement| placement.primary_slot_id != primary.slot_id)
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "history preparation cannot omit or reassign a foreign primary or removed target",
            ));
        }
        if self.state.histories.is_empty()
            || self.state.versions.len() > self.config.max_entries
            || self.state.histories.len() > self.config.max_entries
        {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "backup history closure is empty or exceeds the admitted entry budget",
            ));
        }
        // Whole-pool history selection keeps every directory repository and its
        // nested repositories together. There is no per-file target allocator.
        Ok(())
    }

    fn backup_history_plans(&self) -> Result<Vec<PreparedHistoryPackage>> {
        self.state
            .histories
            .keys()
            .map(|history| self.prepare_backup_history(history))
            .collect()
    }
}

fn describe(plans: &[PreparedHistoryPackage]) -> Result<(BTreeMap<String, String>, u64, u64)> {
    let mut roots = BTreeMap::new();
    let mut files = 0u64;
    let mut bytes = 0u64;
    for plan in plans {
        roots.insert(plan.history_id().into(), plan.manifest_hash().into());
        files = files
            .checked_add(plan.body_count())
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "backup file count overflow"))?;
        bytes = bytes
            .checked_add(plan.body_bytes()?)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "backup size overflow"))?;
    }
    Ok((roots, files, bytes))
}

fn reject_overlap(engine: &Engine, target: &DiskRoot) -> Result<()> {
    if target.path().starts_with(&engine.root)
        || engine.root.starts_with(target.path())
        || engine.config.disks.iter().any(|disk| {
            target.path().starts_with(&disk.root) || disk.root.starts_with(target.path())
        })
    {
        return Err(Error::new(
            ErrorCode::UnsafePath,
            "backup target overlaps a live pool or system root",
        ));
    }
    Ok(())
}

fn reserve_target(target: &DiskRoot, job: &Job) -> Result<()> {
    named_target(target, job)?;
    let intent = target.path().join(TARGET_INTENT);
    if target.optional_metadata(&intent)?.is_none() {
        let entries = target
            .entries(target.path())?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        // Only interrupted atomic-write names can precede the target intent.
        // Retain them in place; no unowned user name is adopted or deleted.
        if entries
            .iter()
            .any(|name| !platform_io::fs_safety::AtomicFile::is_temporary_name(name))
        {
            return Err(Error::new(
                ErrorCode::ObjectCollision,
                "unreserved target contains unrelated entries",
            ));
        }
    }
    crate::history_package::install_preparation_bytes(target, &intent, &canonical_json(job)?)
}

fn named_target(target: &DiskRoot, job: &Job) -> Result<()> {
    if PhysicalBinding::of(&DiskRoot::open(target.path())?)? != job.target {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "backup target path no longer names its held directory",
        ));
    }
    Ok(())
}

fn spawn(
    engine: &Engine,
    job: Job,
    system: DiskRoot,
    target: DiskRoot,
    lock: AdvisoryLock,
    plans: Vec<PreparedHistoryPackage>,
    snapshot: snapshot::PreparedSnapshot,
) -> Result<Worker> {
    let locks = engine.retain_writer_locks()?;
    let state = Arc::new(Mutex::new(job.status()));
    let stop = Arc::new(AtomicBool::new(false));
    let progress = Arc::clone(&state);
    let cancellation = Arc::clone(&stop);
    let frozen = job.clone();
    let thread = std::thread::Builder::new().name("zxs-backup-preparation".into()).spawn(move || {
        let _locks = locks; let _target_lock = lock;
        // Only this worker writes progress while alive; join may close its
        // diagnostic after termination. Never hold the cache lock across I/O.
        let update = |action: &dyn Fn(&mut BackupPreparationStatus)| -> Result<()> {
            let mut snapshot = {
                let status = progress.lock().expect("invariant: backup status poisoned");
                status.clone()
            };
            action(&mut snapshot);
            let persisted = system.write_json(&system.path().join(PROGRESS), &snapshot, true);
            *progress.lock().expect("invariant: backup status poisoned") = snapshot;
            persisted
        };
        let result = execution::copy_target(&frozen, &target, plans, &snapshot, &cancellation, &update)
            .and_then(|_| update(&|status| {
                status.phase = BackupPreparationPhase::HistoryPrepared;
                status.detail = "complete history and published State snapshot verified; control archive/database installation and activation are not implemented".into();
            }));
        if let Err(error) = result {
            let persisted = update(&|status| { status.phase = BackupPreparationPhase::Blocked;
                status.error_code = Some(error.code()); status.detail = error.to_string(); });
            if let Err(persistence) = persisted {
                let mut status = progress.lock().expect("invariant: backup status poisoned");
                status.detail = format!("{error}; progress persistence failed: {persistence}");
            }
        }
    })?;
    Ok(Worker {
        job,
        batch: None,
        state,
        stop,
        thread: Some(thread),
        targets: Vec::new(),
    })
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/backup_preparation_status_tests.rs"]
mod tests;
