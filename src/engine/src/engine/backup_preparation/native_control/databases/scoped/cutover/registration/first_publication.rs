//! Retain the actual reserved first Publication on every selected target, even
//! when no AVAILABLE version exists to carry a route-local publication proof.
//! This is not active Current/Config or a target writer/source-unfence permit.
use super::*;
use crate::engine::backup_preparation::native_control::databases::materials::{
    self, Namespace, PreparedFiles,
};
use crate::engine::membership_transition::{
    AdmittedBackupDecision, BackupFirstPublicationHash, BackupFirstPublicationReport,
    PreparedBackupFirstCandidate, PreparedBackupFirstPublication,
};

const DIRECTORY: &str = "backup-generation-first-publication";
const COMPLETE: &str = "generation-first-publication-retained.json";
const PUBLICATION: &str = "publication.json";
const SEAL: &str = "publication-seal.json";
const MAX_NATIVE: usize = 64 * 1024 * 1024;
const MAX_DESCRIPTOR: usize = 16 * 1024;

/// All selected targets contain the same real original first Publication and
/// complete local payload proofs. This describes reserved historical evidence,
/// not current source health, runtime membership or permission to write.
#[derive(Clone, Debug)]
pub struct BackupTargetPublicationReport {
    pub publication: BackupFirstPublicationReport,
    pub targets: usize,
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
    source_fence_hash: &'a str,
    decision_hash: &'a str,
    authority_hash: &'a str,
    candidate_hash: &'a str,
    publication_hash: &'a str,
    snapshot_hash: &'a str,
    #[serialization(with = "storage_contract::u64_string")]
    first_sequence: u64,
    targets: usize,
    files: BTreeMap<&'static str, FileReference>,
}
#[derive(Serialize)]
struct Completion<'a> {
    format: &'static str,
    decision_hash: &'a str,
    publication_hash: &'a str,
    manifest_hash: &'a str,
    targets: usize,
}
struct Plan {
    evidence: PreparedBackupFirstPublication,
    manifest: Vec<u8>,
    completion: Vec<u8>,
    report: BackupTargetPublicationReport,
}
fn bounded(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > MAX_DESCRIPTOR {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "Backup Publication descriptor exceeds 16 KiB",
        ));
    }
    Ok(bytes)
}
impl Plan {
    fn new(
        prepared: &PreparedBackupRegistry,
        evidence: PreparedBackupFirstPublication,
    ) -> Result<Self> {
        let report = evidence.report();
        let files = [
            (PUBLICATION, evidence.publication_bytes()),
            (SEAL, evidence.seal_bytes()),
        ];
        let manifest = bounded(&Manifest {
            format: "zxs-backup-first-publication",
            source_fence_hash: prepared.source_fence_hash(),
            decision_hash: report.decision_hash.as_str(),
            authority_hash: &report.authority_hash,
            candidate_hash: &report.candidate_hash,
            publication_hash: &report.publication_hash,
            snapshot_hash: &report.snapshot_hash,
            first_sequence: report.first_sequence,
            targets: prepared.targets.len(),
            files: files
                .iter()
                .map(|(name, bytes)| {
                    (
                        *name,
                        FileReference {
                            bytes: bytes.len(),
                            digest: blake3::hash(bytes).to_hex().to_string(),
                        },
                    )
                })
                .collect(),
        })?;
        let completion = bounded(&Completion {
            format: "zxs-backup-first-publication-retained",
            decision_hash: report.decision_hash.as_str(),
            publication_hash: &report.publication_hash,
            manifest_hash: blake3::hash(&manifest).to_hex().as_str(),
            targets: prepared.targets.len(),
        })?;
        let report = BackupTargetPublicationReport {
            publication: report.clone(),
            targets: prepared.targets.len(),
            metadata_bytes_per_target: (files.iter().map(|(_, bytes)| bytes.len()).sum::<usize>()
                + manifest.len()
                + completion.len()) as u64,
        };
        Ok(Self {
            evidence,
            manifest,
            completion,
            report,
        })
    }
    fn load(
        prepared: &PreparedBackupRegistry,
        candidate: &PreparedBackupFirstCandidate,
        expected: &BackupFirstPublicationHash,
        root: &DiskRoot,
    ) -> Result<Self> {
        let directory = root.path().join(DIRECTORY);
        let evidence = PreparedBackupFirstPublication::from_trusted_bytes(
            prepared,
            candidate,
            expected,
            &root.read_bytes(&directory.join(PUBLICATION), MAX_NATIVE)?,
            &root.read_bytes(&directory.join(SEAL), MAX_NATIVE)?,
        )?;
        Self::new(prepared, evidence)
    }
    fn files(&self) -> Result<PreparedFiles<'_>> {
        PreparedFiles::new(
            Namespace::GenerationFirstPublication,
            self.manifest.clone(),
            [
                (
                    PathBuf::from(PUBLICATION),
                    self.evidence.publication_bytes(),
                ),
                (PathBuf::from(SEAL), self.evidence.seal_bytes()),
            ],
        )
    }
    fn capacity(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        materials::require_capacity(root, &self.files()?, stop)?;
        let path = root.path().join(COMPLETE);
        if preparation_path_exists(root, &path)?
            && root.read_bytes(&path, MAX_DESCRIPTOR)? != self.completion
        {
            return Err(Error::new(
                ErrorCode::ObjectCollision,
                "another Backup Publication completion is preserved; no replacement",
            ));
        }
        Ok(())
    }
    fn verify(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        materials::verify(root, &self.files()?, stop)?;
        if root.read_bytes(&root.path().join(COMPLETE), MAX_DESCRIPTOR)? != self.completion {
            return Err(invalid(
                "Backup target lacks its exact original Publication completion",
            ));
        }
        Ok(())
    }
}

fn install(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    candidate: &PreparedBackupFirstCandidate,
    plan: Plan,
    stop: &AtomicBool,
) -> Result<BackupTargetPublicationReport> {
    recovery::require_isolated(&prepared.targets)?;
    decision.verify_targets(prepared, stop)?;
    plan.evidence.verify_targets(prepared, candidate, stop)?;
    // Preflight ALL destinations before any new file. An incomplete/colliding
    // target is never replaced with another target or treated as a smaller set.
    for target in &prepared.targets {
        plan.capacity(&target.root, stop)?;
    }
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        recovery::require_isolated(&prepared.targets)?;
        prepared.recheck_names()?;
        materials::install(&target.root, &plan.files()?, stop)?;
    }
    // Re-read the actual per-target body and existing native proofs, not just
    // the copied descriptor. This is also enforced for detached continuation.
    plan.evidence.verify_targets(prepared, candidate, stop)?;
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        recovery::require_isolated(&prepared.targets)?;
        prepared.recheck_names()?;
        install_preparation_bytes(
            &target.root,
            &target.root.path().join(COMPLETE),
            &plan.completion,
        )?;
        plan.verify(&target.root, stop)?;
    }
    decision.verify_targets(prepared, stop)?;
    recovery::require_isolated(&prepared.targets)?;
    Ok(plan.report)
}

/// Live caller supplied evidence only after the real all-original Publication
/// barrier. A prospective Candidate/Finalize cannot construct this capability.
pub(super) fn prepare(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    candidate: &PreparedBackupFirstCandidate,
    publication: PreparedBackupFirstPublication,
    stop: &AtomicBool,
) -> Result<BackupTargetPublicationReport> {
    install(
        prepared,
        decision,
        candidate,
        Plan::new(prepared, publication)?,
        stop,
    )
}

pub(super) fn observe(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupTargetPublicationReport> {
    let (_, candidate) = first_candidate::read_verified(prepared, decision, stop)?;
    let first = Plan::load(prepared, &candidate, expected, &prepared.targets[0].root)?;
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        first.verify(&target.root, stop)?;
    }
    first.evidence.verify_targets(prepared, &candidate, stop)?;
    decision.verify_targets(prepared, stop)?;
    recovery::require_isolated(&prepared.targets)?;
    Ok(first.report)
}

/// Copy ONLY the already retained exact original publication evidence. There
/// must be an independently authenticated complete witness on an original
/// selected target. No member journal, earlier proof or new choice is written.
pub(super) fn resume(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupTargetPublicationReport> {
    let (_, candidate) = first_candidate::read_verified(prepared, decision, stop)?;
    let mut witness = None;
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        if preparation_path_exists(&target.root, &target.root.path().join(COMPLETE))? {
            let plan = Plan::load(prepared, &candidate, expected, &target.root)?;
            plan.verify(&target.root, stop)?;
            // Every purported complete replica is validated; a corrupt one is
            // not skipped merely because a healthy sibling might exist.
            if witness.is_none() {
                witness = Some(plan);
            }
        }
    }
    let plan = witness.ok_or_else(|| {
        Error::new(
            ErrorCode::RecoveryRequired,
            "no complete original target Publication witness; original source task must resume",
        )
    })?;
    install(prepared, decision, &candidate, plan, stop)
}
