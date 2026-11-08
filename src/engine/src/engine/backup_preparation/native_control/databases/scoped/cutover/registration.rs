//! One closed registry plus complete entity placement projection. Selection is
//! committed by original native decision slots, never by this target JSON alone.
use super::*;
use crate::container_storage::backup_entities::EntityPlacement;
use crate::engine::membership_transition::BackupChoiceContext;
use installation::{Target, Task as InstallationTask};

mod published_registry;
pub use published_registry::{
    BackupNamespacePage, BackupNamespaceScope, BackupPublishedRegistryReport,
};
mod configuration;
pub use configuration::BackupConfigurationReport;
mod first_publication;
pub use first_publication::BackupTargetPublicationReport;
mod first_candidate;
pub use first_candidate::BackupFirstCandidateReport;
mod generation_authority;
pub use generation_authority::BackupGenerationAuthorityReport;
mod generation_control;
pub use generation_control::BackupGenerationControlReport;
mod generation_inputs;
pub use generation_inputs::BackupGenerationPreparationReport;
mod recovery;
mod runtime;
mod writer_routes;
pub use recovery::{
    BackupDecidedTargets, BackupRecoveredContent, BackupRegistryRecoveryReport,
    open_decided_backup_targets,
};
pub(super) use runtime::resume;
pub use writer_routes::{
    BackupDirectoryWriterRoute, BackupFileWriterRoute, BackupWriterDestination,
    BackupWriterRoutesReport,
};
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

#[derive(Clone, Serialize, Deserialize)]
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
impl BackupRegistryProjection {
    /// Exact metadata-only Config projection. The retained old terminal stays
    /// registered (including any original metadata vote); replacement of data
    /// routes never silently becomes a membership removal or a disk Stop.
    pub(in crate::engine) fn configuration(
        &self,
        original: &crate::ValidatedConfig,
    ) -> Result<storage_contract::Config> {
        if self.original_configuration_hash != object_hash("backup-preparation-config", original)?
            || canonical_json(&self.retained_original_disks)? != canonical_json(&original.disks)?
        {
            return Err(invalid(
                "Backup Config projection changes its original fixed disks",
            ));
        }
        let mut config: storage_contract::Config = (**original).clone();
        for target in &self.targets {
            let identity = &target.node.identity;
            if identity.pool_id != original.pool_id || identity.binding_generation != 1 {
                return Err(invalid(
                    "Backup Config target is not a fresh original selected binding",
                ));
            }
            config.disks.push(DiskConfig {
                slot_id: identity.slot_id.clone(),
                disk_id: identity.disk_id.clone(),
                root: target.root.clone(),
                level: target.node.level,
                upstream_slot_id: Some(target.node.upstream_slot_id.clone()),
                metadata_member: false,
                binding_generation: identity.binding_generation,
                // Physical free space is admitted separately by actual target
                // verification. No unrequested logical capacity is invented.
                capacity_bytes: None,
            });
        }
        Ok(config)
    }

    /// Deterministic metadata recipe, not I/O admission or published State.
    /// Both the pre-decision routing snapshot and the first native candidate
    /// use the same complete whole-unit projection.
    pub(in crate::engine) fn project_unpublished_routing(
        &self,
        evidence: &BackupSourceFenceEvidence,
        source: &State,
        stop: &AtomicBool,
    ) -> Result<State> {
        generation_inputs::routing::project_frozen(source, evidence, self, stop)
    }

    pub(in crate::engine) fn selected_nodes(&self) -> Vec<BackupPreparedTopologyNode> {
        self.targets
            .iter()
            .map(|target| target.node.clone())
            .collect()
    }

    /// Pure cold closure of the selected topology and physical target bindings.
    /// This grants no filesystem I/O: actual image admission remains required.
    pub(in crate::engine) fn verify_frozen_selection(
        &self,
        evidence: &BackupSourceFenceEvidence,
        fence_hash: &str,
    ) -> Result<()> {
        let frozen = &evidence.task;
        let jobs = frozen.selection().jobs();
        if self.format != FORMAT
            || self.source_fence_hash != fence_hash
            || self.original_state_hash != evidence.state_hash()
            || self.original_configuration_hash != frozen.selection().first().configuration_hash
            || self.source != frozen.topology.source
            || self.replaced_terminal != frozen.topology.replaced
            || canonical_json(&self.retained_original_disks)?
                != canonical_json(&frozen.original.disks)?
            || self.targets.len() != jobs.len()
            || self.targets.len() != frozen.topology.nodes.len()
        {
            return Err(invalid(
                "Backup authority registry differs from its original frozen task",
            ));
        }
        let mut units = None;
        for ((target, node), job) in self.targets.iter().zip(&frozen.topology.nodes).zip(jobs) {
            if target.node != *node
                || target.physical != job.target
                || target.root != job.request.target_paths[0]
                || target.image_manifest_hash.len() != 64
                || !target
                    .image_manifest_hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(invalid(
                    "Backup authority substitutes a selected target or entity image",
                ));
            }
            let relative = target.entities.relative_to(&node.source_prefix)?;
            if units.as_ref().is_some_and(|first| first != &relative) {
                return Err(invalid(
                    "Backup authority splits whole units across selected targets",
                ));
            }
            units = Some(relative);
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
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
    /// A locked, still-isolated selected root for its exact new identity. This
    /// is only a payload-proof destination, never DiskResources admission.
    pub(in crate::engine) fn payload_target(
        &self,
        slot: &str,
        generation: u64,
    ) -> Result<Option<(&DiskRoot, &DiskRegistrationIdentity)>> {
        let Some(index) = self
            .projection
            .targets
            .iter()
            .position(|target| target.node.identity.slot_id == slot)
        else {
            return Ok(None);
        };
        let identity = &self.projection.targets[index].node.identity;
        if identity.binding_generation != generation {
            return Err(invalid(
                "first Backup payload substitutes a target binding generation",
            ));
        }
        self.recheck_names()?;
        Ok(Some((&self.targets[index].root, identity)))
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
