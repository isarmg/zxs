//! Native-format unpublished routing snapshots after the original Backup
//! decision. No success receipt, active generation marker or writer is forged.
use super::*;
use crate::engine::backup_preparation::native_control::databases::materials::{
    self, Namespace, PreparedFiles,
};
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupRegistryDecisionHash};
use crate::journal::prepare_snapshot;

mod routing;

const FORMAT: &str = "zxs-backup-generation-routing-inputs";
const COMPLETE: &str = "generation-inputs-prepared.json";
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_OBJECTS: usize = 4096;

/// Observed unpublished metadata, not a publication/activation receipt. The
/// projected snapshot has no newly invented operation and cannot activate I/O.
#[derive(Clone, Debug)]
pub struct BackupGenerationPreparationReport {
    pub operation_id: OperationId,
    pub decision_hash: BackupRegistryDecisionHash,
    pub inputs_hash: String,
    pub routing_snapshot_hash: String,
    pub intended_first_sequence: u64,
    pub targets: usize,
    pub whole_files: usize,
    pub directory_repositories: usize,
    pub retained_versions: usize,
    pub metadata_objects: usize,
    pub metadata_bytes_per_target: u64,
}

#[derive(Serialize)]
struct Descriptor {
    bytes: usize,
    digest: String,
}
#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    decision_hash: &'a str,
    source_fence_hash: &'a str,
    original_state_hash: &'a str,
    original_snapshot: &'a str,
    writer_routes_hash: &'a str,
    #[serialization(with = "storage_contract::u64_string")]
    intended_first_sequence: u64,
    routing_state_hash: String,
    routing_snapshot_hash: &'a str,
    // These exact addresses still live under activation-image. Installing the
    // future serving locators is a separate missing native activation step.
    whole_units: &'a EntityPlacement,
    targets: &'a [BackupPreparedTopologyNode],
    objects: BTreeMap<&'a PathBuf, Descriptor>,
}
#[derive(Serialize)]
struct Completion<'a> {
    format: &'static str,
    decision_hash: &'a str,
    inputs_hash: &'a str,
    routing_snapshot_hash: &'a str,
    targets: usize,
}

struct Inputs {
    files: BTreeMap<PathBuf, Vec<u8>>,
    manifest: Vec<u8>,
    complete: Vec<u8>,
    report: BackupGenerationPreparationReport,
}
impl Inputs {
    fn capture(
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        routes: &writer_routes::Routes,
        index: usize,
        stop: &AtomicBool,
    ) -> Result<Self> {
        preparation_checkpoint(stop)?;
        let job = &prepared.task.selection().jobs()[index];
        let source = snapshot::read(&prepared.targets[index].root, job, stop)?;
        if object_hash("backup-prepared-state", source.state())?
            != prepared.source_evidence().state_hash()
        {
            return Err(invalid(
                "generation base differs from the originally decided complete State",
            ));
        }
        let after = routing::project(&source, routes, stop)?;
        // Encode the actual native full-State map roots. This is not a portable
        // HistoryManifest, so permanent operations/principals and private native
        // fields are preserved rather than silently filtered out.
        let snapshot = prepare_snapshot(&after, MAX_BYTES)?;
        let mut files = BTreeMap::new();
        let mut bytes = 0usize;
        snapshot.visit_prepared_objects(&mut |kind, hash, body| {
            preparation_checkpoint(stop)?;
            bytes = bytes.checked_add(body.len()).ok_or_else(budget)?;
            if files.len() == MAX_OBJECTS || bytes > MAX_BYTES {
                return Err(budget());
            }
            let path = PathBuf::from("objects")
                .join(kind)
                .join(format!("{hash}.json"));
            if let Some(previous) = files.insert(path, body.to_vec())
                && previous != body
            {
                return Err(invalid(
                    "native generation snapshot conflicts at one object address",
                ));
            }
            Ok(())
        })?;
        let routes_report = routes.report();
        let decision_hash = decision.hash();
        let first_sequence = decision.intended_first_sequence()?;
        let manifest = bounded(&Manifest {
            format: FORMAT,
            decision_hash: decision_hash.as_str(),
            source_fence_hash: prepared.source_fence_hash(),
            original_state_hash: prepared.source_evidence().state_hash(),
            original_snapshot: &job.snapshot,
            writer_routes_hash: &routes_report.material_hash,
            intended_first_sequence: first_sequence,
            routing_state_hash: object_hash("backup-unpublished-routing-state", &after)?,
            routing_snapshot_hash: &snapshot.hash,
            whole_units: routes.units(),
            targets: &routes_report.targets,
            objects: files
                .iter()
                .map(|(path, bytes)| {
                    (
                        path,
                        Descriptor {
                            bytes: bytes.len(),
                            digest: blake3::hash(bytes).to_hex().to_string(),
                        },
                    )
                })
                .collect(),
        })?;
        let hash = blake3::hash(&manifest).to_hex().to_string();
        let complete = bounded(&Completion {
            format: "zxs-backup-generation-routing-inputs-prepared",
            decision_hash: decision_hash.as_str(),
            inputs_hash: &hash,
            routing_snapshot_hash: &snapshot.hash,
            targets: prepared.targets.len(),
        })?;
        if bytes
            .checked_add(manifest.len())
            .and_then(|total| total.checked_add(complete.len()))
            .ok_or_else(budget)?
            > MAX_BYTES
        {
            return Err(budget());
        }
        let report = BackupGenerationPreparationReport {
            operation_id: decision.operation(),
            decision_hash,
            inputs_hash: hash,
            routing_snapshot_hash: snapshot.hash.clone(),
            intended_first_sequence: first_sequence,
            targets: prepared.targets.len(),
            whole_files: routes_report.whole_files_per_target,
            directory_repositories: routes_report.directory_repositories_per_target,
            retained_versions: routes_report.retained_versions_per_target,
            metadata_objects: files.len(),
            metadata_bytes_per_target: bytes as u64,
        };
        Ok(Self {
            files,
            manifest,
            complete,
            report,
        })
    }
    fn files(&self) -> Result<PreparedFiles<'_>> {
        PreparedFiles::new(
            Namespace::BackupGeneration,
            self.manifest.clone(),
            self.files
                .iter()
                .map(|(path, bytes)| (path.clone(), bytes.as_slice())),
        )
    }
    fn require_capacity(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        materials::require_capacity(root, &self.files()?, stop)?;
        let path = root.path().join(COMPLETE);
        if preparation_path_exists(root, &path)?
            && root.read_bytes(&path, self.complete.len())? != self.complete
        {
            return Err(Error::new(
                ErrorCode::ObjectCollision,
                "generation preparation completion differs; old material retained",
            ));
        }
        // The material store retains 16 MiB of headroom; completion is bounded
        // to 1 MiB and is installed only after every complete copy reads back.
        Ok(())
    }
    fn verify(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        materials::verify(root, &self.files()?, stop)?;
        if root.read_bytes(&root.path().join(COMPLETE), self.complete.len())? != self.complete {
            return Err(invalid(
                "generation completion is not the exact full selected routing snapshot",
            ));
        }
        Ok(())
    }
}

fn bounded(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > 1024 * 1024 {
        return Err(budget());
    }
    Ok(bytes)
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "backup generation inputs exceed 4096 objects / 64 MiB complete bytes / 1 MiB manifest / 16 MiB routing changes",
    )
}

fn capture_all(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    stop: &AtomicBool,
) -> Result<Inputs> {
    let routes = writer_routes::observe(prepared, decision, stop)?;
    let first = Inputs::capture(prepared, decision, &routes, 0, stop)?;
    for index in 1..prepared.targets.len() {
        // Never fill one target from another target's decoded State. Its own
        // source proof reconstructs the same entire unpublished write set.
        let current = Inputs::capture(prepared, decision, &routes, index, stop)?;
        if current.manifest != first.manifest || current.complete != first.complete {
            return Err(invalid(
                "selected targets derive different complete generation inputs",
            ));
        }
    }
    prepared.recheck_names()?;
    Ok(first)
}

pub(super) fn prepare(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    stop: &AtomicBool,
) -> Result<BackupGenerationPreparationReport> {
    let inputs = capture_all(prepared, decision, stop)?;
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        inputs.require_capacity(&target.root, stop)?;
    }
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        recovery::require_isolated(&prepared.targets)?;
        prepared.recheck_names()?;
        materials::install(&target.root, &inputs.files()?, stop)?;
    }
    decision.verify_targets(prepared, stop)?;
    for target in &prepared.targets {
        materials::verify(&target.root, &inputs.files()?, stop)?;
    }
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        recovery::require_isolated(&prepared.targets)?;
        prepared.recheck_names()?;
        install_preparation_bytes(
            &target.root,
            &target.root.path().join(COMPLETE),
            &inputs.complete,
        )?;
        inputs.verify(&target.root, stop)?;
    }
    recovery::require_isolated(&prepared.targets)?;
    prepared.recheck_names()?;
    Ok(inputs.report)
}

pub(super) fn observe(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    stop: &AtomicBool,
) -> Result<BackupGenerationPreparationReport> {
    let inputs = capture_all(prepared, decision, stop)?;
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        inputs.verify(&target.root, stop)?;
    }
    recovery::require_isolated(&prepared.targets)?;
    prepared.recheck_names()?;
    Ok(inputs.report)
}
