//! Durable inputs and target-local acceptances for a future common cutover.
//! These names are deliberately outside membership decision namespaces. A
//! prepared topology never grants I/O, a vote, an active route or a new receipt.
use super::*;
use crate::history_package::{
    install_preparation_bytes, preparation_checkpoint, preparation_path_exists,
};
use std::collections::BTreeSet;

mod fence;
mod installation;
mod registration;
pub(in crate::engine) use fence::BackupSourceFenceEvidence;
pub(in crate::engine::backup_preparation) use installation::resume_fenced;
pub use registration::{
    BackupCommonDatabasesReport, BackupConfigurationReport, BackupCurrentReadReport,
    BackupDatabaseAlias, BackupDatabaseAliasesReport, BackupDatabaseFile, BackupDatabaseRange,
    BackupDatabaseRead, BackupDatabaseScope, BackupDecidedTargets, BackupDirectoryWriterRoute,
    BackupFileWriterRoute, BackupFirstCandidateReport, BackupGenerationAuthorityReport,
    BackupGenerationControlReport, BackupGenerationPreparationReport, BackupNamespacePage,
    BackupNamespaceScope, BackupNativeMetadataReport, BackupOriginalDatabaseReport,
    BackupPublishedRegistryReport, BackupPublishedScopeTargetReport, BackupPublishedScopesReport,
    BackupRecoveredContent, BackupRegistryRecoveryReport, BackupRootDatabasesReport,
    BackupServingContent, BackupServingImagesReport, BackupServingRead, BackupServingTargetReport,
    BackupSystemDatabaseReport, BackupTargetLocatorReport, BackupTargetPublicationReport,
    BackupTargetSealReport, BackupTargetSealsReport, BackupTargetWriterIoReport,
    BackupTargetWriterReport, BackupTargetWritersReport, BackupWriterDestination,
    BackupWriterIoReport, BackupWriterRoutesReport, open_decided_backup_targets,
};
pub(in crate::engine) use registration::{
    BackupRegistryProjection, MAX_DECISION_BYTES, PreparedBackupRegistry,
};
mod runtime;
mod topology;
pub(super) use runtime::resume;
use topology::Topology;

const FORMAT: &str = "zxs-backup-cutover-inputs";
const TASK: &str = "backup-preparation/cutover-inputs.json";
const INTENT: &str = "cutover-inputs.json";
const ACCEPTANCE: &str = "cutover-target-prepared.json";
const COMPLETE: &str = "cutover-inputs-prepared.json";

/// One fresh identity for each already selected physical target, in the exact
/// original order. An upstream is either the original source or another target
/// in this same request. No target or replica count is selected implicitly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BackupTopologyTarget {
    pub disk_id: String,
    pub slot_id: String,
    pub upstream_slot_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BackupCutoverPreparationRequest {
    pub operation_id: OperationId,
    pub principal: String,
    pub targets: Vec<BackupTopologyTarget>,
}

/// Prospective identity and hop-by-hop namespace prefix. It is not a registered
/// disk, and its root remains owned by the original isolated preparation task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BackupPreparedTopologyNode {
    pub identity: DiskRegistrationIdentity,
    pub upstream_slot_id: String,
    pub level: u32,
    pub name: String,
    pub source_prefix: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct CutoverTask {
    format: String,
    scoped: ScopedTask,
    request: BackupCutoverPreparationRequest,
    original: Config,
    topology: Topology,
}
impl CutoverTask {
    fn selection(&self) -> &Selection {
        &self.scoped.objects.native.selection
    }
    fn validate(&self) -> Result<()> {
        self.scoped.validate()?;
        let request = &self.selection().first().request;
        if self.format != FORMAT
            || self.request.operation_id != request.operation_id
            || self.request.principal != request.principal
        {
            return Err(invalid(
                "cutover inputs must retain the original actor and operation",
            ));
        }
        let original = crate::ValidatedConfig::try_from(self.original.clone())?;
        if object_hash("backup-preparation-config", &original)?
            != self.selection().first().configuration_hash
            || self.topology
                != Topology::derive(&original, self.selection(), &self.request.targets)?
        {
            return Err(invalid(
                "cutover topology differs from its frozen original source and complete selected roots",
            ));
        }
        Ok(())
    }
    fn bytes(&self) -> Result<Vec<u8>> {
        bounded(self)
    }
    fn read(system: &DiskRoot) -> Result<Self> {
        let task: Self =
            crate::journal::decode_proof(&system.read_bytes(&system.path().join(TASK), MAX_TASK)?)?;
        task.validate()?;
        if task.scoped.bytes()? != ScopedTask::read(system)?.bytes()? {
            return Err(invalid(
                "cutover task no longer extends its exact scoped database task",
            ));
        }
        Ok(task)
    }
}

/// Every node retains the complete same pool unit. The status does not certify
/// complete historical DB scopes, active entity installation or final authority.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BackupCutoverPreparationReport {
    pub format: String,
    pub task_hash: String,
    pub scoped: BackupScopedDatabasePreparationReport,
    pub original_source: DiskRegistrationIdentity,
    pub replaced_terminal: DiskRegistrationIdentity,
    pub targets: Vec<BackupPreparedTopologyNode>,
    pub whole_file_units_per_target: usize,
    pub directory_repositories_per_target: usize,
    pub retained_versions_per_target: usize,
}

#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
struct TargetAcceptance<'a> {
    format: &'static str,
    task_hash: &'a str,
    report_hash: String,
    node: &'a BackupPreparedTopologyNode,
    physical: &'a PhysicalBinding,
}
fn bounded(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > MAX_TASK {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "cutover inputs exceed the complete metadata budget",
        ));
    }
    Ok(bytes)
}
fn report(
    task: &CutoverTask,
    scoped: BackupScopedDatabasePreparationReport,
    roots: &[&DiskRoot],
    stop: &AtomicBool,
) -> Result<BackupCutoverPreparationReport> {
    if scoped.task_hash != object_hash(super::FORMAT, &task.scoped)? {
        return Err(invalid(
            "cutover inputs refer to a different scoped preparation",
        ));
    }
    let mut counts = None;
    for (root, job) in roots.iter().zip(task.selection().jobs()) {
        preparation_checkpoint(stop)?;
        // Complete admitted State, not a UI selection or only current HEADs.
        // The preceding scoped verifier independently authenticated the native
        // prefix, all history/body bytes and all object DBs on this same root.
        let state = snapshot::read(root, job, stop)?;
        let state = state.state();
        let mut files = BTreeSet::new();
        for version in state.versions.values() {
            preparation_checkpoint(stop)?;
            files.insert(&version.file_id);
        }
        let current = (
            files.len(),
            state.directory_git_repositories.len(),
            state.versions.len(),
        );
        if counts.is_some_and(|old| old != current) {
            return Err(invalid(
                "one selected topology target omits whole file or directory units",
            ));
        }
        counts = Some(current);
    }
    let (files, directories, versions) =
        counts.expect("invariant: all selected roots are nonempty");
    Ok(BackupCutoverPreparationReport {
        format: FORMAT.into(),
        task_hash: object_hash(FORMAT, task)?,
        scoped,
        original_source: task.topology.source.clone(),
        replaced_terminal: task.topology.replaced.clone(),
        targets: task.topology.nodes.clone(),
        whole_file_units_per_target: files,
        directory_repositories_per_target: directories,
        retained_versions_per_target: versions,
    })
}
fn acceptance(
    task: &CutoverTask,
    report: &BackupCutoverPreparationReport,
    index: usize,
) -> Result<Vec<u8>> {
    bounded(&TargetAcceptance {
        format: "zxs-backup-cutover-target-inputs",
        task_hash: &report.task_hash,
        report_hash: object_hash("backup-cutover-preparation-report", report)?,
        node: &task.topology.nodes[index],
        physical: &task.selection().jobs()[index].target,
    })
}

fn verify_at(
    task: &CutoverTask,
    roots: &[&DiskRoot],
    stop: &AtomicBool,
) -> Result<BackupCutoverPreparationReport> {
    let scoped = super::verify_at(&task.scoped, roots, stop)?;
    let observed = report(task, scoped, roots, stop)?;
    let intent = task.bytes()?;
    let completed = bounded(&observed)?;
    for (index, (root, job)) in roots.iter().zip(task.selection().jobs()).enumerate() {
        preparation_checkpoint(stop)?;
        if root.read_bytes(&root.path().join(INTENT), MAX_TASK)? != intent
            || root.read_bytes(&root.path().join(ACCEPTANCE), MAX_TASK)?
                != acceptance(task, &observed, index)?
            || root.read_bytes(&root.path().join(COMPLETE), MAX_TASK)? != completed
        {
            return Err(invalid(
                "cutover inputs lack an exact independently checked whole-set acceptance",
            ));
        }
        named_target(root, job)?;
    }
    Ok(observed)
}

/// Inspect only the complete original target set. The returned descriptive
/// report cannot be passed as a vote or installed as a running Config.
pub fn offline_check_backup_cutover_inputs(
    paths: &[PathBuf],
) -> Result<BackupCutoverPreparationReport> {
    if !(1..=8).contains(&paths.len()) {
        return Err(invalid("all selected topology targets are required"));
    }
    let roots = paths
        .iter()
        .map(|path| DiskRoot::open(path))
        .collect::<Result<Vec<_>>>()?;
    let task: CutoverTask = crate::journal::decode_proof(
        &roots[0].read_bytes(&roots[0].path().join(INTENT), MAX_TASK)?,
    )?;
    task.validate()?;
    if !task
        .selection()
        .jobs()
        .iter()
        .map(|job| &job.request.target_paths[0])
        .eq(paths.iter())
    {
        return Err(invalid("cutover target order or physical path set changed"));
    }
    verify_at(
        &task,
        &roots.iter().collect::<Vec<_>>(),
        &AtomicBool::new(false),
    )
}
