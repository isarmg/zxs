//! An explicit, durable extension of a completed material task. Old completion
//! markers keep their original meaning; this stage does not enroll or switch.
use super::*;
use std::path::{Component, Path};

mod archive;
pub(in crate::engine) use archive::generation_objects::BackupNativeObjects;
mod databases;
pub use databases::{
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
mod runtime;
pub use archive::BackupNativeControlReport;
pub(in crate::engine::backup_preparation) use databases::resume_fenced;
pub use databases::{
    BackupCutoverPreparationReport, BackupCutoverPreparationRequest,
    BackupObjectDatabasePreparationReport, BackupPreparedTopologyNode,
    BackupScopedDatabasePreparationReport, BackupTopologyTarget,
    offline_check_backup_cutover_inputs, offline_check_backup_object_databases,
    offline_check_backup_scoped_databases,
};
pub(in crate::engine) use databases::{
    BackupRegistryProjection, BackupSourceFenceEvidence, MAX_DECISION_BYTES, PreparedBackupRegistry,
};

const FORMAT: &str = "zxs-backup-native-preparation";
const TASK: &str = "backup-preparation/native-control.json";
const INTENT: &str = "native-preparation.json";
const COMPLETE: &str = "native-control-prepared.json";
const MAX_TASK: usize = 4 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serialization(
    tag = "kind",
    content = "preparation",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
enum Selection {
    Single(Job),
    Batch(batch::Batch),
}
impl Selection {
    fn jobs(&self) -> &[Job] {
        match self {
            Self::Single(job) => std::slice::from_ref(job),
            Self::Batch(batch) => batch.jobs(),
        }
    }
    fn first(&self) -> &Job {
        &self.jobs()[0]
    }
    fn validate(&self) -> Result<()> {
        match self {
            Self::Batch(batch) => batch.validate(),
            Self::Single(job) => {
                if job.format != super::FORMAT || job.request.target_paths.len() != 1 {
                    return Err(invalid("unsupported original history preparation"));
                }
                let path = &job.request.target_paths[0];
                if !path.is_absolute()
                    || path
                        .components()
                        .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
                {
                    return Err(invalid(
                        "native preparation requires the exact normalized target path",
                    ));
                }
                Engine::require_actor(&job.request.principal)
            }
        }
    }
    fn read(system: &DiskRoot) -> Result<Self> {
        let single =
            crate::history_package::preparation_path_exists(system, &system.path().join(JOB))?;
        let batch = crate::history_package::preparation_path_exists(
            system,
            &system.path().join(batch::BATCH_JOB),
        )?;
        let selection = match (single, batch) {
            (true, false) => Self::Single(crate::journal::decode_proof(
                &system.read_bytes(&system.path().join(JOB), MAX_TASK)?,
            )?),
            (false, true) => Self::Batch(batch::read_batch(system)?),
            _ => {
                return Err(invalid(
                    "one exact single or batch preparation must exist; no selection is guessed",
                ));
            }
        };
        selection.validate()?;
        Ok(selection)
    }
    fn status(&self) -> BackupPreparationStatus {
        let mut status = match self {
            Self::Single(job) => job.status(),
            Self::Batch(batch) => batch.status(),
        };
        status.completed_files = status.total_files;
        status.completed_bytes = status.total_bytes;
        status.phase = BackupPreparationPhase::Verifying;
        status.detail = "preparing original native control evidence; history bytes remain prepared, not activated".into();
        status
    }
    fn verify_material(
        &self,
        roots: &[&DiskRoot],
        stop: &AtomicBool,
    ) -> Result<Vec<BackupPreparationReport>> {
        if roots.len() != self.jobs().len() {
            return Err(invalid(
                "native preparation must retain all selected complete copies",
            ));
        }
        match self {
            Self::Single(job) => {
                let root = roots[0];
                named_target(root, job)?;
                let report = inspection::check_at(root, stop)?;
                if report.job_hash != object_hash("backup-preparation-job", job)? {
                    return Err(invalid(
                        "history marker belongs to a different original task",
                    ));
                }
                Ok(vec![report])
            }
            Self::Batch(batch) => {
                let report = batch::verify_targets(batch, roots, stop)?;
                let expected = canonical_json(&report)?;
                for root in roots {
                    if root.read_bytes(&root.path().join(batch::BATCH_COMPLETE), MAX_TASK)?
                        != expected
                    {
                        return Err(invalid(
                            "the complete original batch marker is required before native preparation",
                        ));
                    }
                }
                Ok(report.targets)
            }
        }
    }
    fn idle(self, error: Error, phase: BackupPreparationPhase) -> Worker {
        let mut status = self.status();
        status.phase = phase;
        status.error_code = Some(error.code());
        status.detail = error.to_string();
        let job = self.first().clone();
        let batch = match self {
            Self::Single(_) => None,
            Self::Batch(batch) => Some(batch),
        };
        Worker {
            job,
            batch,
            state: Arc::new(Mutex::new(status)),
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            targets: Vec::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct NativeTask {
    format: String,
    selection: Selection,
    archive_hash: String,
}
impl NativeTask {
    fn validate(&self) -> Result<()> {
        self.selection.validate()?;
        if self.format != FORMAT
            || self.archive_hash.len() != 64
            || !self
                .archive_hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid("invalid retained native preparation task"));
        }
        Ok(())
    }
    fn bytes(&self) -> Result<Vec<u8>> {
        let bytes = canonical_json(self)?;
        if bytes.len() > MAX_TASK {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "native task exceeds its durable intent budget",
            ));
        }
        Ok(bytes)
    }
    fn read(system: &DiskRoot) -> Result<Self> {
        let task: Self =
            crate::journal::decode_proof(&system.read_bytes(&system.path().join(TASK), MAX_TASK)?)?;
        task.validate()?;
        if canonical_json(&task.selection)? != canonical_json(&Selection::read(system)?)? {
            return Err(invalid(
                "native task no longer binds the original complete target selection",
            ));
        }
        Ok(task)
    }
}

fn invalid(detail: &str) -> Error {
    Error::new(ErrorCode::ConfigConflict, detail)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BackupNativeTargetReport {
    pub material: BackupPreparationReport,
    pub control: BackupNativeControlReport,
}

/// Target-local evidence from the entire explicit target selection. This is not
/// a membership receipt, new route, accepted foreign chain or activation token.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BackupNativePreparationReport {
    pub format: String,
    pub task_hash: String,
    pub targets: Vec<BackupNativeTargetReport>,
}

fn verify_targets(
    task: &NativeTask,
    roots: &[&DiskRoot],
    stop: &AtomicBool,
) -> Result<BackupNativePreparationReport> {
    let materials = task.selection.verify_material(roots, stop)?;
    let intent = task.bytes()?;
    let mut reports = Vec::with_capacity(roots.len());
    for ((root, job), material) in roots.iter().zip(task.selection.jobs()).zip(materials) {
        crate::history_package::preparation_checkpoint(stop)?;
        if root.read_bytes(&root.path().join(INTENT), MAX_TASK)? != intent {
            return Err(invalid(
                "native target has a different whole-selection intent",
            ));
        }
        let control = archive::read(root, job, &task.archive_hash, stop)?;
        if control.state_hash != material.state_hash
            || control.snapshot_hash != material.snapshot_hash
            || control.sequence != material.sequence
        {
            return Err(invalid(
                "native prefix and complete history material have different boundaries",
            ));
        }
        named_target(root, job)?;
        reports.push(BackupNativeTargetReport { material, control });
    }
    Ok(BackupNativePreparationReport {
        format: FORMAT.into(),
        task_hash: object_hash(FORMAT, task)?,
        targets: reports,
    })
}

/// Read only selected target roots. All original histories, full State pages,
/// original journals and native receipt-bound objects must independently pass.
/// No source fallback, repair, disk registration or authority change is allowed.
pub fn offline_check_backup_native_preparation(
    paths: &[PathBuf],
) -> Result<BackupNativePreparationReport> {
    if !(1..=8).contains(&paths.len()) {
        return Err(invalid(
            "the complete selected native target set is required",
        ));
    }
    let roots = paths
        .iter()
        .map(|path| DiskRoot::open(path))
        .collect::<Result<Vec<_>>>()?;
    let task: NativeTask = crate::journal::decode_proof(
        &roots[0].read_bytes(&roots[0].path().join(INTENT), MAX_TASK)?,
    )?;
    task.validate()?;
    if !task
        .selection
        .jobs()
        .iter()
        .map(|job| &job.request.target_paths[0])
        .eq(paths.iter())
    {
        return Err(invalid(
            "native target set/order differs from the immutable preparation",
        ));
    }
    let held = roots.iter().collect::<Vec<_>>();
    verify_completed(&task, &held, &AtomicBool::new(false))
}

fn verify_completed(
    task: &NativeTask,
    roots: &[&DiskRoot],
    stop: &AtomicBool,
) -> Result<BackupNativePreparationReport> {
    let report = verify_targets(task, roots, stop)?;
    let bytes = canonical_json(&report)?;
    if bytes.len() > MAX_TASK {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "native completion exceeds its reader budget",
        ));
    }
    for (root, job) in roots.iter().zip(task.selection.jobs()) {
        crate::history_package::preparation_checkpoint(stop)?;
        if root.read_bytes(&root.path().join(COMPLETE), MAX_TASK)? != bytes {
            return Err(invalid(
                "native completion is missing or differs from verified material",
            ));
        }
        named_target(root, job)?;
    }
    Ok(report)
}

pub(super) fn resume(engine: &mut Engine, system: &DiskRoot) -> Result<bool> {
    if databases::resume(engine, system)? {
        return Ok(true);
    }
    runtime::resume(engine, system)
}
