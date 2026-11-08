//! Complete file/directory SQLite preparation, not the missing system/disk
//! historical scopes or a membership/placement cutover decision.
use super::*;
use crate::container_storage::backup_databases::PreparedDatabases;

mod materials;
mod runtime;
mod scoped;
pub use scoped::{
    BackupDecidedTargets, BackupRecoveredContent, BackupRegistryRecoveryReport,
    open_decided_backup_targets,
};
mod store;
pub(in crate::engine::backup_preparation) use scoped::resume_fenced;
pub use scoped::{
    BackupCutoverPreparationReport, BackupCutoverPreparationRequest, BackupPreparedTopologyNode,
    BackupScopedDatabasePreparationReport, BackupTopologyTarget,
    offline_check_backup_cutover_inputs, offline_check_backup_scoped_databases,
};
pub(in crate::engine) use scoped::{
    BackupRegistryProjection, BackupSourceFenceEvidence, MAX_DECISION_BYTES, PreparedBackupRegistry,
};

const FORMAT: &str = "zxs-backup-object-databases";
const TASK: &str = "backup-preparation/object-databases.json";
const INTENT: &str = "object-databases-intent.json";
const COMPLETE: &str = "object-databases-prepared.json";

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct DatabaseTask {
    format: String,
    native: NativeTask,
    state_hash: String,
}
impl DatabaseTask {
    fn bytes(&self) -> Result<Vec<u8>> {
        let bytes = canonical_json(self)?;
        if bytes.len() > MAX_TASK {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "object database task exceeds its bound",
            ));
        }
        Ok(bytes)
    }
    fn validate(&self) -> Result<()> {
        self.native.validate()?;
        if self.format != FORMAT
            || self.state_hash.len() != 64
            || !self
                .state_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid("invalid whole-target database task"));
        }
        Ok(())
    }
    fn read(system: &DiskRoot) -> Result<Self> {
        let task: Self =
            crate::journal::decode_proof(&system.read_bytes(&system.path().join(TASK), MAX_TASK)?)?;
        task.validate()?;
        if canonical_json(&task.native)? != canonical_json(&NativeTask::read(system)?)? {
            return Err(invalid(
                "object database task no longer binds the original native stage",
            ));
        }
        Ok(task)
    }
    fn check_native(&self, report: &BackupNativePreparationReport) -> Result<()> {
        if report.task_hash != object_hash(super::FORMAT, &self.native)?
            || report
                .targets
                .iter()
                .any(|target| target.material.state_hash != self.state_hash)
        {
            return Err(invalid(
                "object databases and native evidence have different complete States",
            ));
        }
        Ok(())
    }
}

/// All counts are per selected complete target, not sharded or summed into an
/// apparent replica policy. This observation is never an activation capability.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BackupObjectDatabasePreparationReport {
    pub format: String,
    pub task_hash: String,
    pub native: BackupNativePreparationReport,
    pub database_manifest_hash: String,
    pub files_per_target: usize,
    pub directories_per_target: usize,
    #[serialization(with = "storage_contract::u64_string")]
    pub database_bytes_per_target: u64,
}

fn report(
    task: &DatabaseTask,
    native: BackupNativePreparationReport,
    prepared: &PreparedDatabases,
) -> Result<BackupObjectDatabasePreparationReport> {
    task.check_native(&native)?;
    Ok(BackupObjectDatabasePreparationReport {
        format: FORMAT.into(),
        task_hash: object_hash(FORMAT, task)?,
        native,
        database_manifest_hash: blake3::hash(&prepared.manifest()?).to_hex().to_string(),
        files_per_target: prepared.file_count(),
        directories_per_target: prepared.directory_count(),
        database_bytes_per_target: prepared.total_bytes(),
    })
}

fn completion_bytes(report: &BackupObjectDatabasePreparationReport) -> Result<Vec<u8>> {
    let bytes = canonical_json(report)?;
    if bytes.len() > MAX_TASK {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "object database completion exceeds reader budget",
        ));
    }
    Ok(bytes)
}

/// Independently reconstruct every object database from each target's admitted
/// full State, then read its real SQLite bytes. No source, stored self hash,
/// completion flag or healthy peer can replace the missing target evidence.
pub fn offline_check_backup_object_databases(
    paths: &[PathBuf],
) -> Result<BackupObjectDatabasePreparationReport> {
    if !(1..=8).contains(&paths.len()) {
        return Err(invalid("the entire original target selection is required"));
    }
    let roots = paths
        .iter()
        .map(|path| DiskRoot::open(path))
        .collect::<Result<Vec<_>>>()?;
    let task: DatabaseTask = crate::journal::decode_proof(
        &roots[0].read_bytes(&roots[0].path().join(INTENT), MAX_TASK)?,
    )?;
    task.validate()?;
    if !task
        .native
        .selection
        .jobs()
        .iter()
        .map(|job| &job.request.target_paths[0])
        .eq(paths)
    {
        return Err(invalid(
            "object database target set or ordering differs from the original",
        ));
    }
    let stop = AtomicBool::new(false);
    verify_at(&task, &roots.iter().collect::<Vec<_>>(), &stop)
}

fn verify_at(
    task: &DatabaseTask,
    roots: &[&DiskRoot],
    stop: &AtomicBool,
) -> Result<BackupObjectDatabasePreparationReport> {
    let native = verify_completed(&task.native, roots, stop)?;
    task.check_native(&native)?;
    let intent = task.bytes()?;
    let mut completed = None;
    for (root, job) in roots.iter().zip(task.native.selection.jobs()) {
        if root.read_bytes(&root.path().join(INTENT), MAX_TASK)? != intent {
            return Err(invalid(
                "object database target belongs to a different complete selection",
            ));
        }
        let state = snapshot::read(root, job, stop)?;
        let prepared = PreparedDatabases::capture(&state, stop)?;
        store::verify(root, &prepared, stop)?;
        let observed = report(task, native.clone(), &prepared)?;
        let bytes = completion_bytes(&observed)?;
        if root.read_bytes(&root.path().join(COMPLETE), MAX_TASK)? != bytes {
            return Err(invalid(
                "object database completion differs from target-local verified bytes",
            ));
        }
        if completed
            .as_ref()
            .is_some_and(|(_, expected)| expected != &bytes)
        {
            return Err(invalid("selected targets have different database closures"));
        }
        named_target(root, job)?;
        completed = Some((observed, bytes));
    }
    Ok(completed
        .expect("invariant: nonempty complete target selection")
        .0)
}

pub(in crate::engine::backup_preparation) use runtime::resume;
