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
mod execution;
mod inspection;
mod native_control;
pub(in crate::engine) use native_control::BackupNativeObjects;
pub use native_control::{
    BackupCutoverPreparationReport, BackupCutoverPreparationRequest, BackupNativeControlReport,
    BackupNativePreparationReport, BackupNativeTargetReport, BackupObjectDatabasePreparationReport,
    BackupPreparedTopologyNode, BackupScopedDatabasePreparationReport, BackupTopologyTarget,
    offline_check_backup_cutover_inputs, offline_check_backup_native_preparation,
    offline_check_backup_object_databases, offline_check_backup_scoped_databases,
};
pub use native_control::{
    BackupDecidedTargets, BackupDirectoryWriterRoute, BackupFileWriterRoute,
    BackupGenerationControlReport, BackupGenerationPreparationReport, BackupRecoveredContent,
    BackupRegistryRecoveryReport, BackupWriterDestination, BackupWriterRoutesReport,
    open_decided_backup_targets,
};
pub(in crate::engine) use native_control::{
    BackupRegistryProjection, BackupSourceFenceEvidence, MAX_DECISION_BYTES, PreparedBackupRegistry,
};
mod snapshot;
pub use batch::{BackupPreparationBatchReport, offline_check_backup_preparation_batch};
pub use inspection::{BackupPreparationReport, offline_check_backup_preparation};

const FORMAT: &str = "zxs-backup-history-preparation";
const JOB: &str = "backup-preparation/job.json";
const PROGRESS: &str = "backup-preparation/progress.json";
const TARGET_INTENT: &str = "backup-preparation.json";
const RESERVE: u64 = 16 * 1024 * 1024;

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
        let system = DiskRoot::open(&self.root)?;
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
    })
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/backup_preparation_status_tests.rs"]
mod tests;
