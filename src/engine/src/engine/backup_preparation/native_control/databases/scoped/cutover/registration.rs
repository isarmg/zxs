//! One closed registry plus complete entity placement projection. Selection is
//! committed by original native decision slots, never by this target JSON alone.
use super::*;
use crate::container_storage::backup_entities::EntityPlacement;
use crate::engine::membership_transition::BackupChoiceContext;
use installation::{Target, Task as InstallationTask};

mod recovery;
mod runtime;
pub use recovery::{
    BackupDecidedTargets, BackupRecoveredContent, BackupRegistryRecoveryReport,
    open_decided_backup_targets,
};
pub(super) use runtime::resume;
const FORMAT: &str = "zxs-backup-registry-placement";
const TASK: &str = "backup-preparation/registry-placement.json";
const DIRECTORY: &str = "backup-registry-placement";
const INPUTS: &str = "inputs.json";
const DECISION: &str = "decision.json";
pub(in crate::engine) const MAX_DECISION_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Task {
    format: String,
    installation: InstallationTask,
}
impl Task {
    fn selection(&self) -> &Selection {
        self.installation.selection()
    }
    fn bytes(&self) -> Result<Vec<u8>> {
        encode(self)
    }
    fn read(system: &DiskRoot) -> Result<Self> {
        let task: Self = crate::journal::decode_proof(
            &system.read_bytes(&system.path().join(TASK), MAX_DECISION_BYTES)?,
        )?;
        task.installation.validate()?;
        if task.format != FORMAT
            || task.installation.bytes()? != InstallationTask::read(system)?.bytes()?
        {
            return Err(invalid(
                "registry decision task differs from original entity installation",
            ));
        }
        Ok(task)
    }
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct BackupRegistryProjection {
    format: String,
    source_fence_hash: String,
    original_state_hash: String,
    original_configuration_hash: String,
    source: DiskRegistrationIdentity,
    replaced_terminal: DiskRegistrationIdentity,
    // Retention is part of the same decision. No delete/exit effect is implied.
    retained_original_disks: Vec<DiskConfig>,
    targets: Vec<TargetProjection>,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct TargetProjection {
    node: BackupPreparedTopologyNode,
    physical: PhysicalBinding,
    root: PathBuf,
    image_manifest_hash: String,
    entities: EntityPlacement,
}

/// Unforgeable within other modules: construction requires complete target-local
/// admission and exact image readback while every original target lock is held.
pub(in crate::engine) struct PreparedBackupRegistry {
    task: Task,
    targets: Vec<Target>,
    projection: BackupRegistryProjection,
}
impl PreparedBackupRegistry {
    fn capture(task: Task, targets: Vec<Target>, stop: &AtomicBool) -> Result<Self> {
        installation::verify_installed(&task.installation, &targets, stop)?;
        let projection = project(&task, &targets, stop)?;
        encode(&projection)?;
        Ok(Self {
            task,
            targets,
            projection,
        })
    }
    pub(in crate::engine) fn source_evidence(&self) -> &BackupSourceFenceEvidence {
        &self.task.installation.evidence
    }
    pub(in crate::engine) fn decision_copy(&self) -> Result<Option<Vec<u8>>> {
        self.recheck_names()?;
        for target in &self.targets {
            inventory(&target.root)?;
            let path = target.root.path().join(DIRECTORY).join(DECISION);
            if preparation_path_exists(&target.root, &path)? {
                return target.root.read_bytes(&path, MAX_DECISION_BYTES).map(Some);
            }
        }
        Ok(None)
    }
    pub(in crate::engine) fn require_decision_copies(
        &self,
        inputs: &[u8],
        decision: &[u8],
        stop: &AtomicBool,
    ) -> Result<()> {
        for target in &self.targets {
            preparation_checkpoint(stop)?;
            inventory(&target.root)?;
            for (name, expected) in [(INPUTS, inputs), (DECISION, decision)] {
                if target.root.read_bytes(
                    &target.root.path().join(DIRECTORY).join(name),
                    MAX_DECISION_BYTES,
                )? != expected
                {
                    return Err(invalid(
                        "selected target has another or incomplete native decision",
                    ));
                }
            }
        }
        self.recheck_names()
    }
    pub(in crate::engine) fn source_fence_hash(&self) -> &str {
        &self.task.installation.source_fence_hash
    }
    pub(in crate::engine) fn projection(&self) -> &BackupRegistryProjection {
        &self.projection
    }
    pub(in crate::engine) fn recheck_names(&self) -> Result<()> {
        for (target, job) in self.targets.iter().zip(self.task.selection().jobs()) {
            named_target(&target.root, job)?;
        }
        Ok(())
    }
    pub(in crate::engine) fn verify(&self, stop: &AtomicBool) -> Result<()> {
        installation::verify_installed(&self.task.installation, &self.targets, stop)?;
        if encode(&project(&self.task, &self.targets, stop)?)? != encode(&self.projection)? {
            return Err(invalid(
                "complete registry or placement changed after target admission",
            ));
        }
        self.recheck_names()
    }
    pub(in crate::engine) fn require_capacity(&self, inputs: &[u8], decision: &[u8]) -> Result<()> {
        self.recheck_names()?;
        for target in &self.targets {
            let root = &target.root;
            inventory(root)?;
            let mut required = RESERVE;
            for (name, bytes) in [(INPUTS, inputs), (DECISION, decision)] {
                let path = root.path().join(DIRECTORY).join(name);
                if preparation_path_exists(root, &path)? {
                    if root.read_bytes(&path, MAX_DECISION_BYTES)? != bytes {
                        return Err(invalid(
                            "retained registry decision bytes conflict; nothing replaced",
                        ));
                    }
                } else {
                    required = required
                        .checked_add(bytes.len() as u64)
                        .ok_or_else(|| invalid("registry decision size overflows"))?;
                }
            }
            if root.available()? < required {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "every selected target must reserve its own complete registry decision",
                ));
            }
        }
        Ok(())
    }
    pub(in crate::engine) fn install_inputs(&self, bytes: &[u8], stop: &AtomicBool) -> Result<()> {
        self.install(INPUTS, bytes, stop)
    }
    pub(in crate::engine) fn install_decision(
        &self,
        bytes: &[u8],
        stop: &AtomicBool,
    ) -> Result<()> {
        self.install(DECISION, bytes, stop)
    }
    fn install(&self, name: &str, bytes: &[u8], stop: &AtomicBool) -> Result<()> {
        for target in &self.targets {
            preparation_checkpoint(stop)?;
            self.recheck_names()?;
            inventory(&target.root)?;
            install_preparation_bytes(
                &target.root,
                &target.root.path().join(DIRECTORY).join(name),
                bytes,
            )?;
        }
        for target in &self.targets {
            preparation_checkpoint(stop)?;
            if target.root.read_bytes(
                &target.root.path().join(DIRECTORY).join(name),
                MAX_DECISION_BYTES,
            )? != bytes
            {
                return Err(invalid("registry decision copy differs after installation"));
            }
        }
        self.recheck_names()
    }
}
fn project(task: &Task, targets: &[Target], stop: &AtomicBool) -> Result<BackupRegistryProjection> {
    let installation = &task.installation;
    let frozen = &installation.evidence.task;
    let mut entries = Vec::with_capacity(targets.len());
    for (index, target) in targets.iter().enumerate() {
        let job = &task.selection().jobs()[index];
        entries.push(installation::with_image(
            installation,
            target,
            index,
            stop,
            |image| {
                Ok(TargetProjection {
                    node: frozen.topology.nodes[index].clone(),
                    physical: job.target.clone(),
                    root: job.request.target_paths[0].clone(),
                    image_manifest_hash: image.manifest_hash(),
                    entities: image.placement().clone(),
                })
            },
        )?);
    }
    Ok(BackupRegistryProjection {
        format: FORMAT.into(),
        source_fence_hash: installation.source_fence_hash.clone(),
        original_state_hash: installation.evidence.state_hash().into(),
        original_configuration_hash: task.selection().first().configuration_hash.clone(),
        source: frozen.topology.source.clone(),
        replaced_terminal: frozen.topology.replaced.clone(),
        retained_original_disks: frozen.original.disks.clone(),
        targets: entries,
    })
}
fn inventory(root: &DiskRoot) -> Result<()> {
    let path = root.path().join(DIRECTORY);
    if !preparation_path_exists(root, &path)? {
        return Ok(());
    }
    let held = root.private_child(&path)?;
    for entry in held.entries(held.path())? {
        let entry = entry.map_err(fs_error)?;
        if !matches!(entry.to_str(), Some(INPUTS | DECISION)) {
            return Err(Error::new(
                ErrorCode::ObjectCollision,
                "unknown registry decision material retained; no cleanup or adoption",
            ));
        }
        held.open_regular(&held.path().join(entry))?;
    }
    Ok(())
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > MAX_DECISION_BYTES {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "complete registry plus placement decision exceeds 32 MiB",
        ));
    }
    Ok(bytes)
}
