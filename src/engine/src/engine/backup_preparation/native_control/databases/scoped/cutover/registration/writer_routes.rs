//! The selected whole-unit fanout, materialized after the real native choice.
//! This is input for a future generation publisher, not an active writer lease.
//! No Config/Current/Placement is forged from this preparation material.
use super::*;
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupRegistryDecisionHash};
use std::path::Path;

mod store;

const FORMAT: &str = "zxs-backup-selected-writer-routes";
const MAX_BYTES: usize = 32 * 1024 * 1024;

/// Observed preparation, not a publication receipt. The native source fence
/// still owns write exclusion even when all target route materials are present.
#[derive(Clone, Debug)]
pub struct BackupWriterRoutesReport {
    pub operation_id: OperationId,
    pub decision_hash: BackupRegistryDecisionHash,
    pub material_hash: String,
    pub targets: Vec<BackupPreparedTopologyNode>,
    pub retained_source_chain: Vec<DiskRegistrationIdentity>,
    pub retained_replaced_terminal: DiskRegistrationIdentity,
    pub whole_files_per_target: usize,
    pub directory_repositories_per_target: usize,
    pub retained_versions_per_target: usize,
}

/// A selected target's exact existing image home. It is not a path supplied by
/// the caller and cannot be converted into an ordinary Engine write handle.
#[derive(Clone, Debug)]
pub struct BackupWriterDestination {
    pub target: DiskRegistrationIdentity,
    pub upstream_slot_id: String,
    pub level: u32,
    pub image_relative_home: PathBuf,
}

/// Every retained version travels as one file on EVERY explicitly selected
/// target. The destination vector is a fanout set, not an ordered linear route.
#[derive(Clone, Debug)]
pub struct BackupFileWriterRoute {
    pub decision_hash: BackupRegistryDecisionHash,
    pub history_tree_id: String,
    pub file_id: String,
    pub directory_id: Option<String>,
    pub version_ids: BTreeSet<String>,
    pub destinations: Vec<BackupWriterDestination>,
}

/// Parent and nested child repositories use the same complete destination set;
/// the parent relation and relative entity home are preserved on every copy.
#[derive(Clone, Debug)]
pub struct BackupDirectoryWriterRoute {
    pub decision_hash: BackupRegistryDecisionHash,
    pub history_tree_id: String,
    pub directory_id: String,
    pub outer_repository_id: String,
    pub parent_directory_id: Option<String>,
    pub destinations: Vec<BackupWriterDestination>,
}

#[derive(Serialize)]
struct Material {
    format: &'static str,
    decision_hash: String,
    source_fence_hash: String,
    original_state_hash: String,
    original_configuration_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    source_sequence: u64,
    source_snapshot: String,
    retained_source_chain: Vec<DiskRegistrationIdentity>,
    retained_replaced_terminal: DiskRegistrationIdentity,
    targets: Vec<BackupPreparedTopologyNode>,
    // Relative to each target's own immediate-upstream prefix. This shared
    // table is admitted only if every independent physical image agrees.
    units: EntityPlacement,
}

#[derive(Serialize)]
struct Completion<'a> {
    format: &'static str,
    decision_hash: &'a str,
    material_hash: &'a str,
    selected_targets: usize,
}

/// Constructed only from an admitted decision and fully read-back images.
/// Stored route JSON is compared to recomputed bytes, never trusted as input.
pub(super) struct Routes {
    material: Material,
    bytes: Vec<u8>,
    completion: Vec<u8>,
    report: BackupWriterRoutesReport,
}

impl Routes {
    fn capture(
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let projection = &prepared.projection;
        let first = &projection.targets[0];
        let units = first.entities.relative_to(&first.node.source_prefix)?;
        for target in &projection.targets[1..] {
            preparation_checkpoint(stop)?;
            if target.entities.relative_to(&target.node.source_prefix)? != units {
                return Err(invalid(
                    "selected targets split a file, repository, parent/child boundary or retained version set",
                ));
            }
        }
        let material = Material {
            format: FORMAT,
            decision_hash: decision.hash().as_str().into(),
            source_fence_hash: projection.source_fence_hash.clone(),
            original_state_hash: projection.original_state_hash.clone(),
            original_configuration_hash: projection.original_configuration_hash.clone(),
            source_sequence: prepared.task.selection().first().request.expected_sequence,
            source_snapshot: prepared.task.selection().first().snapshot.clone(),
            retained_source_chain: source_chain(prepared)?,
            retained_replaced_terminal: projection.replaced_terminal.clone(),
            targets: projection
                .targets
                .iter()
                .map(|target| target.node.clone())
                .collect(),
            units,
        };
        let bytes = encode(&material)?;
        let material_hash = object_hash(FORMAT, &material)?;
        let completion = encode(&Completion {
            format: "zxs-backup-selected-writer-routes-prepared",
            decision_hash: &material.decision_hash,
            material_hash: &material_hash,
            selected_targets: material.targets.len(),
        })?;
        let report = BackupWriterRoutesReport {
            operation_id: decision.operation(),
            decision_hash: decision.hash(),
            material_hash,
            targets: material.targets.clone(),
            retained_source_chain: material.retained_source_chain.clone(),
            retained_replaced_terminal: material.retained_replaced_terminal.clone(),
            whole_files_per_target: material.units.files.len(),
            directory_repositories_per_target: material.units.directories.len(),
            retained_versions_per_target: material
                .units
                .files
                .iter()
                .map(|file| file.version_ids.len())
                .sum(),
        };
        Ok(Self {
            material,
            bytes,
            completion,
            report,
        })
    }

    pub(super) fn report(&self) -> BackupWriterRoutesReport {
        self.report.clone()
    }

    pub(super) fn file(&self, history: &str, file: &str) -> Result<BackupFileWriterRoute> {
        let home = self
            .material
            .units
            .files
            .iter()
            .find(|home| home.history_tree_id == history && home.file_id == file)
            .ok_or_else(|| invalid("file is not in the decided whole-unit writer material"))?;
        Ok(BackupFileWriterRoute {
            decision_hash: self.report.decision_hash.clone(),
            history_tree_id: home.history_tree_id.clone(),
            file_id: home.file_id.clone(),
            directory_id: home.directory_id.clone(),
            version_ids: home.version_ids.clone(),
            destinations: self.destinations(&home.path),
        })
    }

    pub(super) fn directory(
        &self,
        history: &str,
        directory: &str,
    ) -> Result<BackupDirectoryWriterRoute> {
        let home = self
            .material
            .units
            .directories
            .iter()
            .find(|home| home.history_tree_id == history && home.directory_id == directory)
            .ok_or_else(|| {
                invalid("directory repository is not in the decided whole-unit writer material")
            })?;
        Ok(BackupDirectoryWriterRoute {
            decision_hash: self.report.decision_hash.clone(),
            history_tree_id: home.history_tree_id.clone(),
            directory_id: home.directory_id.clone(),
            outer_repository_id: home.outer_repository_id.clone(),
            parent_directory_id: home.parent_directory_id.clone(),
            destinations: self.destinations(&home.path),
        })
    }

    fn destinations(&self, relative: &Path) -> Vec<BackupWriterDestination> {
        self.material
            .targets
            .iter()
            .map(|target| BackupWriterDestination {
                target: target.identity.clone(),
                upstream_slot_id: target.upstream_slot_id.clone(),
                level: target.level,
                image_relative_home: EntityPlacement::image_base(&target.source_prefix)
                    .join(relative),
            })
            .collect()
    }
}

fn source_chain(prepared: &PreparedBackupRegistry) -> Result<Vec<DiskRegistrationIdentity>> {
    let original = &prepared.task.installation.evidence.task.original;
    let mut disk = original
        .disks
        .iter()
        .find(|disk| disk.slot_id == prepared.projection.source.slot_id)
        .expect("invariant: admitted selection has an original source");
    let mut chain = Vec::new();
    loop {
        chain.push(DiskRegistrationIdentity {
            pool_id: original.pool_id.clone(),
            disk_id: disk.disk_id.clone(),
            slot_id: disk.slot_id.clone(),
            binding_generation: disk.binding_generation,
        });
        let Some(upstream) = &disk.upstream_slot_id else {
            break;
        };
        disk = original
            .disks
            .iter()
            .find(|disk| &disk.slot_id == upstream)
            .expect("invariant: validated original topology reaches its primary");
    }
    if original.disks.len() != chain.len() + 1 {
        return Err(Error::new(
            ErrorCode::UnsupportedOperation,
            "writer material currently requires the complete original single-primary chain and one retained terminal",
        ));
    }
    chain.reverse();
    Ok(chain)
}

fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > MAX_BYTES {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "selected writer-route material exceeds 32 MiB",
        ));
    }
    Ok(bytes)
}

pub(super) fn prepare(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    stop: &AtomicBool,
) -> Result<BackupWriterRoutesReport> {
    recovery::require_isolated(&prepared.targets)?;
    decision.verify_targets(prepared, stop)?;
    let routes = Routes::capture(prepared, decision, stop)?;
    store::prepare(prepared, decision, &routes, stop)?;
    Ok(routes.report())
}

pub(super) fn observe(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    stop: &AtomicBool,
) -> Result<Routes> {
    recovery::require_isolated(&prepared.targets)?;
    decision.verify_targets(prepared, stop)?;
    let routes = Routes::capture(prepared, decision, stop)?;
    store::verify(prepared, &routes, stop)?;
    recovery::require_isolated(&prepared.targets)?;
    prepared.recheck_names()?;
    Ok(routes)
}
