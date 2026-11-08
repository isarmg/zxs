//! Actual SYSTEM-current and original disk-current/history SQLite materials.
//! Not complete historical SYSTEM scope, replacement identities, or cutover.
use super::*;

mod cutover;
pub(in crate::engine::backup_preparation) use cutover::resume_fenced;
pub use cutover::{
    BackupCutoverPreparationReport, BackupCutoverPreparationRequest, BackupPreparedTopologyNode,
    BackupTopologyTarget, offline_check_backup_cutover_inputs,
};
pub use cutover::{
    BackupDecidedTargets, BackupDirectoryWriterRoute, BackupFileWriterRoute,
    BackupFirstCandidateReport, BackupGenerationAuthorityReport, BackupGenerationControlReport,
    BackupGenerationPreparationReport, BackupRecoveredContent, BackupRegistryRecoveryReport,
    BackupTargetPublicationReport, BackupWriterDestination, BackupWriterRoutesReport,
    open_decided_backup_targets,
};
pub(in crate::engine) use cutover::{
    BackupRegistryProjection, BackupSourceFenceEvidence, MAX_DECISION_BYTES, PreparedBackupRegistry,
};
mod projection;
mod runtime;
mod store;
use projection::PreparedScopes;
pub(super) use runtime::resume;

const FORMAT: &str = "zxs-backup-scoped-databases";
const TASK: &str = "backup-preparation/scoped-databases.json";
const INTENT: &str = "scoped-databases-intent.json";
const COMPLETE: &str = "scoped-databases-prepared.json";

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct ScopedTask {
    format: String,
    objects: DatabaseTask,
}
impl ScopedTask {
    fn validate(&self) -> Result<()> {
        self.objects.validate()?;
        if self.format != FORMAT {
            return Err(invalid("unsupported original-scope database task"));
        }
        Ok(())
    }
    fn bytes(&self) -> Result<Vec<u8>> {
        let bytes = canonical_json(self)?;
        if bytes.len() > MAX_TASK {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "scoped database intent exceeds bound",
            ));
        }
        Ok(bytes)
    }
    fn read(system: &DiskRoot) -> Result<Self> {
        let task: Self =
            crate::journal::decode_proof(&system.read_bytes(&system.path().join(TASK), MAX_TASK)?)?;
        task.validate()?;
        if task.objects.bytes()? != DatabaseTask::read(system)?.bytes()? {
            return Err(invalid(
                "scoped task differs from the original complete object task",
            ));
        }
        Ok(task)
    }
}

/// All scopes retain original source identities. Counts do not mean a prepared
/// replacement is registered or that a three-scope authority switch happened.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BackupScopedDatabasePreparationReport {
    pub format: String,
    pub task_hash: String,
    pub objects: BackupObjectDatabasePreparationReport,
    pub scope_manifest_hash: String,
    pub original_disks_per_target: usize,
    pub scoped_databases_per_target: usize,
    #[serialization(with = "storage_contract::u64_string")]
    pub scoped_database_bytes_per_target: u64,
}

fn report(
    task: &ScopedTask,
    objects: BackupObjectDatabasePreparationReport,
    scopes: &PreparedScopes,
) -> Result<BackupScopedDatabasePreparationReport> {
    if objects.task_hash != object_hash(super::FORMAT, &task.objects)? {
        return Err(invalid(
            "scoped databases do not extend the original object preparation",
        ));
    }
    Ok(BackupScopedDatabasePreparationReport {
        format: FORMAT.into(),
        task_hash: object_hash(FORMAT, task)?,
        objects,
        scope_manifest_hash: blake3::hash(&scopes.manifest()?).to_hex().to_string(),
        original_disks_per_target: scopes.disk_count(),
        scoped_databases_per_target: scopes.databases().len(),
        scoped_database_bytes_per_target: scopes.total_bytes(),
    })
}
fn completion_bytes(report: &BackupScopedDatabasePreparationReport) -> Result<Vec<u8>> {
    let bytes = canonical_json(report)?;
    if bytes.len() > MAX_TASK {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "scoped completion exceeds bound",
        ));
    }
    Ok(bytes)
}

/// Reconstruct every original scope from each target's own authenticated native
/// prefix, then compare actual SQLite bytes. No live Engine or system root is
/// consulted, and neither a self digest nor another peer fills missing data.
pub fn offline_check_backup_scoped_databases(
    paths: &[PathBuf],
) -> Result<BackupScopedDatabasePreparationReport> {
    if !(1..=8).contains(&paths.len()) {
        return Err(invalid("all originally selected target paths are required"));
    }
    let roots = paths
        .iter()
        .map(|path| DiskRoot::open(path))
        .collect::<Result<Vec<_>>>()?;
    let task: ScopedTask = crate::journal::decode_proof(
        &roots[0].read_bytes(&roots[0].path().join(INTENT), MAX_TASK)?,
    )?;
    task.validate()?;
    if !task
        .objects
        .native
        .selection
        .jobs()
        .iter()
        .map(|job| &job.request.target_paths[0])
        .eq(paths.iter())
    {
        return Err(invalid(
            "scoped target selection/order differs from original intent",
        ));
    }
    verify_at(
        &task,
        &roots.iter().collect::<Vec<_>>(),
        &AtomicBool::new(false),
    )
}

fn verify_at(
    task: &ScopedTask,
    roots: &[&DiskRoot],
    stop: &AtomicBool,
) -> Result<BackupScopedDatabasePreparationReport> {
    if roots.len() != task.objects.native.selection.jobs().len() {
        return Err(invalid(
            "scoped verification requires every original selected target",
        ));
    }
    let objects = super::verify_at(&task.objects, roots, stop)?;
    let intent = task.bytes()?;
    let mut completion = None;
    for (root, job) in roots.iter().zip(task.objects.native.selection.jobs()) {
        if root.read_bytes(&root.path().join(INTENT), MAX_TASK)? != intent {
            return Err(invalid(
                "target scoped intent belongs to another original task",
            ));
        }
        let scopes = PreparedScopes::capture(root, task, job, stop)?;
        store::verify(root, &scopes, stop)?;
        let observation = report(task, objects.clone(), &scopes)?;
        let bytes = completion_bytes(&observation)?;
        if root.read_bytes(&root.path().join(COMPLETE), MAX_TASK)? != bytes
            || completion
                .as_ref()
                .is_some_and(|(_, expected)| expected != &bytes)
        {
            return Err(invalid(
                "scoped completion does not match all independently verified targets",
            ));
        }
        named_target(root, job)?;
        completion = Some((observation, bytes));
    }
    Ok(completion.expect("invariant: nonempty selected targets").0)
}
