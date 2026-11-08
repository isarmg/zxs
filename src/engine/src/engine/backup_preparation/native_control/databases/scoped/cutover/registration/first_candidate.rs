//! Exact native first-candidate metadata in the isolated target domain. The
//! source's genuine decision is required, but no ACCEPT/FINALIZE/Publication is
//! manufactured from it. Targets do not become voters merely by storing bytes.
use super::*;
use crate::engine::backup_preparation::native_control::databases::materials::{
    self, Namespace, PreparedFiles,
};
use crate::engine::membership_transition::{
    AdmittedBackupDecision, BackupRegistryDecisionHash, PreparedBackupFirstCandidate,
};

const DIRECTORY: &str = "backup-generation-first-candidate";
const COMPLETE: &str = "generation-first-candidate-prepared.json";
const MAX_MANIFEST: usize = 4 * 1024 * 1024;

/// Actual isolated preparation only. No successful OperationResult, active
/// generation, member receipt or permission to release the source fence.
#[derive(Clone, Debug)]
pub struct BackupFirstCandidateReport {
    pub operation_id: OperationId,
    pub decision_hash: BackupRegistryDecisionHash,
    pub authority_hash: String,
    pub candidate_hash: String,
    pub snapshot_hash: String,
    pub intended_first_sequence: u64,
    pub original_metadata_members: Vec<String>,
    pub targets: usize,
    pub available_version_obligations: usize,
    pub metadata_objects: usize,
    pub metadata_bytes_per_target: u64,
}
#[derive(Serialize)]
struct FileReference {
    bytes: usize,
    digest: String,
}
#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    decision_hash: &'a str,
    source_fence_hash: &'a str,
    authority_hash: &'a str,
    candidate_hash: &'a str,
    snapshot_hash: &'a str,
    generation_inputs_hash: &'a str,
    #[serialization(with = "storage_contract::u64_string")]
    first_sequence: u64,
    original_metadata_members: &'a [String],
    available_version_obligations: usize,
    files: BTreeMap<&'a PathBuf, FileReference>,
}
#[derive(Serialize)]
struct Completion<'a> {
    format: &'static str,
    decision_hash: &'a str,
    authority_hash: &'a str,
    candidate_hash: &'a str,
    manifest_hash: &'a str,
    targets: usize,
}
struct Plan {
    candidate: PreparedBackupFirstCandidate,
    manifest: Vec<u8>,
    completion: Vec<u8>,
    report: BackupFirstCandidateReport,
}
fn bounded(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > MAX_MANIFEST {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "first Backup candidate manifest exceeds 4 MiB",
        ));
    }
    Ok(bytes)
}
impl Plan {
    fn capture(
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        inputs: &BackupGenerationPreparationReport,
        index: usize,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let (authority, source) =
            generation_authority::read_candidate_input(prepared, decision, inputs, index, stop)?;
        let candidate = authority.first_candidate(source.state(), stop)?;
        let decision_hash = decision.hash();
        let manifest = bounded(&Manifest {
            format: "zxs-backup-native-first-candidate",
            decision_hash: decision_hash.as_str(),
            source_fence_hash: prepared.source_fence_hash(),
            authority_hash: authority.hash(),
            candidate_hash: candidate.hash(),
            snapshot_hash: candidate.snapshot_hash(),
            generation_inputs_hash: &inputs.inputs_hash,
            first_sequence: candidate.sequence(),
            original_metadata_members: candidate.members(),
            available_version_obligations: candidate.available_versions(),
            files: candidate
                .files()
                .map(|(path, bytes)| {
                    (
                        path,
                        FileReference {
                            bytes: bytes.len(),
                            digest: blake3::hash(bytes).to_hex().to_string(),
                        },
                    )
                })
                .collect(),
        })?;
        let manifest_hash = blake3::hash(&manifest).to_hex().to_string();
        let completion = bounded(&Completion {
            format: "zxs-backup-native-first-candidate-prepared",
            decision_hash: decision_hash.as_str(),
            authority_hash: authority.hash(),
            candidate_hash: candidate.hash(),
            manifest_hash: &manifest_hash,
            targets: prepared.targets.len(),
        })?;
        let report = BackupFirstCandidateReport {
            operation_id: decision.operation(),
            decision_hash,
            authority_hash: authority.hash().into(),
            candidate_hash: candidate.hash().into(),
            snapshot_hash: candidate.snapshot_hash().into(),
            intended_first_sequence: candidate.sequence(),
            original_metadata_members: candidate.members().to_vec(),
            targets: prepared.targets.len(),
            available_version_obligations: candidate.available_versions(),
            metadata_objects: candidate.objects(),
            metadata_bytes_per_target: candidate.bytes() as u64,
        };
        Ok(Self {
            candidate,
            manifest,
            completion,
            report,
        })
    }
    fn files(&self) -> Result<PreparedFiles<'_>> {
        PreparedFiles::new(
            Namespace::GenerationFirstCandidate,
            self.manifest.clone(),
            self.candidate
                .files()
                .map(|(path, bytes)| (path.clone(), bytes)),
        )
    }
    fn capacity(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        materials::require_capacity(root, &self.files()?, stop)?;
        let path = root.path().join(COMPLETE);
        if preparation_path_exists(root, &path)?
            && root.read_bytes(&path, self.completion.len())? != self.completion
        {
            return Err(Error::new(
                ErrorCode::ObjectCollision,
                "another first candidate completion is retained; no overwrite or new operation",
            ));
        }
        // Material-store 16 MiB reserve covers the bounded completion as well.
        Ok(())
    }
    fn verify(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        materials::verify(root, &self.files()?, stop)?;
        self.candidate
            .verify_at(&root.private_child(&root.path().join(DIRECTORY))?, stop)
    }
    fn require_completion(&self, root: &DiskRoot) -> Result<()> {
        if root.read_bytes(&root.path().join(COMPLETE), self.completion.len())? != self.completion {
            return Err(invalid("first candidate has no exact original completion"));
        }
        Ok(())
    }
}

pub(super) fn prepare(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    stop: &AtomicBool,
) -> Result<BackupFirstCandidateReport> {
    recovery::require_isolated(&prepared.targets)?;
    decision.verify_targets(prepared, stop)?;
    let inputs = generation_inputs::observe(prepared, decision, stop)?;
    let first = Plan::capture(prepared, decision, &inputs, 0, stop)?;
    for index in 0..prepared.targets.len() {
        preparation_checkpoint(stop)?;
        if index != 0 {
            let plan = Plan::capture(prepared, decision, &inputs, index, stop)?;
            if plan.manifest != first.manifest {
                return Err(invalid(
                    "targets derive different exact first native candidates",
                ));
            }
        }
        first.capacity(&prepared.targets[index].root, stop)?;
    }
    // Every target independently supplied the original complete State and
    // actual archive before the first no-clobber write of the new candidate.
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        recovery::require_isolated(&prepared.targets)?;
        prepared.recheck_names()?;
        materials::install(&target.root, &first.files()?, stop)?;
    }
    for target in &prepared.targets {
        first.verify(&target.root, stop)?;
    }
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        recovery::require_isolated(&prepared.targets)?;
        prepared.recheck_names()?;
        install_preparation_bytes(
            &target.root,
            &target.root.path().join(COMPLETE),
            &first.completion,
        )?;
        first.require_completion(&target.root)?;
    }
    decision.verify_targets(prepared, stop)?;
    recovery::require_isolated(&prepared.targets)?;
    Ok(first.report)
}

pub(super) fn observe(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    stop: &AtomicBool,
) -> Result<BackupFirstCandidateReport> {
    Ok(read_verified(prepared, decision, stop)?.0)
}

pub(super) fn read_verified(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    stop: &AtomicBool,
) -> Result<(BackupFirstCandidateReport, PreparedBackupFirstCandidate)> {
    recovery::require_isolated(&prepared.targets)?;
    decision.verify_targets(prepared, stop)?;
    let inputs = generation_inputs::observe(prepared, decision, stop)?;
    let mut manifest: Option<Vec<u8>> = None;
    let mut final_plan = None;
    for index in 0..prepared.targets.len() {
        preparation_checkpoint(stop)?;
        let plan = Plan::capture(prepared, decision, &inputs, index, stop)?;
        plan.verify(&prepared.targets[index].root, stop)?;
        plan.require_completion(&prepared.targets[index].root)?;
        if manifest
            .as_ref()
            .is_some_and(|expected| *expected != plan.manifest)
        {
            return Err(invalid(
                "recovered targets do not bind one original first candidate",
            ));
        }
        if index + 1 == prepared.targets.len() {
            final_plan = Some((plan.report, plan.candidate));
        } else {
            manifest = Some(plan.manifest);
        }
    }
    decision.verify_targets(prepared, stop)?;
    recovery::require_isolated(&prepared.targets)?;
    Ok(final_plan.expect("invariant: selected task has one to eight targets"))
}
