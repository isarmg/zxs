//! Actual ordinary target writer-file fencing. Ownership remains with the
//! retained original Engine task; no runtime I/O epoch or active seal is issued.
use super::*;
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupFirstPublicationHash};

mod store;
pub(super) use store::resume_existing;

const INTENT: &str = "backup-preparation/target-writer-fences.json";
const COMPLETE: &str = "backup-target-writer-fences-installed.json";
const MAX_RECORD: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct BackupTargetWriterReport {
    pub identity: DiskRegistrationIdentity,
    #[serialization(with = "storage_contract::u64_string")]
    pub writer_device: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub writer_inode: u64,
}

/// Fresh full-participant proof plus actual target writer ownership. These
/// locks prevent another writer, but do not authorize an Engine publication.
#[derive(Clone, Debug)]
pub struct BackupTargetWritersReport {
    pub native: BackupNativeMetadataReport,
    pub fencing_hash: String,
    pub targets: Vec<BackupTargetWriterReport>,
}

#[derive(Serialize)]
struct TargetBinding<'a> {
    topology: &'a BackupPreparedTopologyNode,
    directory: &'a PhysicalBinding,
}
#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    source_fence_hash: &'a str,
    decision_hash: &'a str,
    publication_hash: &'a str,
    native_installation_hash: &'a str,
    authority_hash: &'a str,
    candidate_hash: &'a str,
    registry: &'a DiskRegistryCommitment,
    targets: Vec<TargetBinding<'a>>,
}
#[derive(Serialize)]
struct Completion<'a> {
    format: &'static str,
    plan_hash: &'a str,
    targets: &'a [BackupTargetWriterReport],
}
struct Plan {
    native: BackupNativeMetadataReport,
    bytes: Vec<u8>,
    hash: String,
}
fn bounded(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > MAX_RECORD {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "Backup target writer descriptor exceeds 64 KiB",
        ));
    }
    Ok(bytes)
}

impl Plan {
    fn capture(
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let native = native_metadata::observe(context, prepared, decision, expected, stop)?;
        let nodes = prepared.projection.selected_nodes();
        let jobs = prepared.task.selection().jobs();
        let bytes = bounded(&Manifest {
            format: "zxs-backup-target-writer-plan",
            source_fence_hash: prepared.source_fence_hash(),
            decision_hash: decision.hash().as_str(),
            publication_hash: expected.as_str(),
            native_installation_hash: &native.installation_hash,
            authority_hash: &native.authority_hash,
            candidate_hash: &native.candidate_hash,
            registry: &native.registry,
            targets: nodes
                .iter()
                .zip(jobs)
                .map(|(topology, job)| TargetBinding {
                    topology,
                    directory: &job.target,
                })
                .collect(),
        })?;
        let hash = blake3::hash(&bytes).to_hex().to_string();
        Ok(Self {
            native,
            bytes,
            hash,
        })
    }

    fn require_intent(&self, system: &DiskRoot) -> Result<()> {
        if system.read_bytes(&system.path().join(INTENT), MAX_RECORD)? != self.bytes {
            return Err(invalid(
                "target writer plan differs from the exact native installation",
            ));
        }
        Ok(())
    }

    fn observe_targets(
        &self,
        prepared: &PreparedBackupRegistry,
    ) -> Result<Vec<BackupTargetWriterReport>> {
        let mut reports = Vec::with_capacity(prepared.targets.len());
        for (target, selected) in prepared.targets.iter().zip(&prepared.projection.targets) {
            let writer = store::observe(target, &self.hash, &selected.node.identity)?;
            reports.push(BackupTargetWriterReport {
                identity: selected.node.identity.clone(),
                writer_device: writer.device,
                writer_inode: writer.inode,
            });
        }
        prepared.recheck_names()?;
        Ok(reports)
    }
}

fn visit(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    stop: &AtomicBool,
    mut action: impl FnMut(&DiskRoot) -> Result<()>,
) -> Result<()> {
    action(context.system)?;
    context.sources.for_each_disk(
        stop,
        |root, _| action(root),
        &prepared.task.selection().first().pool_id,
    )?;
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        action(&target.root)?;
    }
    Ok(())
}

fn completion(plan: &Plan, targets: &[BackupTargetWriterReport]) -> Result<Vec<u8>> {
    bounded(&Completion {
        format: "zxs-backup-target-writer-fences",
        plan_hash: &plan.hash,
        targets,
    })
}

fn require_completion(root: &DiskRoot, expected: &[u8]) -> Result<()> {
    if root.read_bytes(&root.path().join(COMPLETE), MAX_RECORD)? != expected {
        return Err(invalid(
            "target writer completion changed its exact plan or writer identities",
        ));
    }
    Ok(())
}

pub(super) fn install(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupTargetWritersReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    let mut complete = false;
    visit(context, prepared, stop, |root| {
        complete |= preparation_path_exists(root, &root.path().join(COMPLETE))?;
        Ok(())
    })?;
    if complete {
        // The first completion follows ALL target bindings and held locks.
        // A missing file/receipt now is loss, never permission to rebind an inode.
        plan.require_intent(context.system)?;
        let reports = plan.observe_targets(prepared)?;
        let bytes = completion(&plan, &reports)?;
        visit(context, prepared, stop, |root| {
            if preparation_path_exists(root, &root.path().join(COMPLETE))? {
                require_completion(root, &bytes)?;
            }
            Ok(())
        })?;
    } else {
        if preparation_path_exists(context.system, &context.system.path().join(INTENT))? {
            plan.require_intent(context.system)?;
        }
        // Tiny new records still need space on EVERY participant before the
        // first mutation, in addition to the original metadata reserve.
        let required = context
            .sources
            .metadata_reserve_bytes()
            .checked_add(1024 * 1024)
            .ok_or_else(|| {
                Error::new(ErrorCode::ResourceLimit, "Backup writer reserve overflow")
            })?;
        visit(context, prepared, stop, |root| {
            if root.available()? < required {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "Backup writer fencing needs the original metadata reserve plus 1 MiB per root",
                ));
            }
            Ok(())
        })?;
        for (target, selected) in prepared.targets.iter().zip(&prepared.projection.targets) {
            store::preflight(target, &plan.hash, &selected.node.identity)?;
        }
        install_preparation_bytes(
            context.system,
            &context.system.path().join(INTENT),
            &plan.bytes,
        )?;
        for (target, selected) in prepared.targets.iter().zip(&prepared.projection.targets) {
            preparation_checkpoint(stop)?;
            store::install(target, &plan.hash, &selected.node.identity)?;
        }
        plan.observe_targets(prepared)?;
    }
    // Re-observe the actual original votes, all databases, native objects and
    // target bodies before the common completion; lock descriptors stay held.
    let old_plan = plan.bytes;
    let after = Plan::capture(context, prepared, decision, expected, stop)?;
    if after.bytes != old_plan {
        return Err(invalid(
            "Backup native installation changed while target writers were fenced",
        ));
    }
    after.require_intent(context.system)?;
    let targets = after.observe_targets(prepared)?;
    let bytes = completion(&after, &targets)?;
    visit(context, prepared, stop, |root| {
        install_preparation_bytes(root, &root.path().join(COMPLETE), &bytes)
    })?;
    visit(context, prepared, stop, |root| {
        require_completion(root, &bytes)
    })?;
    after.observe_targets(prepared)?;
    Ok(BackupTargetWritersReport {
        native: after.native,
        fencing_hash: blake3::hash(&bytes).to_hex().to_string(),
        targets,
    })
}

pub(super) fn observe(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupTargetWritersReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    plan.require_intent(context.system)?;
    let targets = plan.observe_targets(prepared)?;
    let bytes = completion(&plan, &targets)?;
    visit(context, prepared, stop, |root| {
        require_completion(root, &bytes)
    })?;
    Ok(BackupTargetWritersReport {
        native: plan.native,
        fencing_hash: blake3::hash(&bytes).to_hex().to_string(),
        targets,
    })
}
