//! Actual native predecessor archive on every independently held target. This
//! closes the fenced-original evidence gap, not the new writer publication.
use super::*;
use crate::engine::backup_preparation::native_control::databases::materials::{
    self, Namespace, PreparedFiles,
};
use crate::engine::membership_transition::{
    AdmittedBackupDecision, BackupFrozenControl, BackupRegistryDecisionHash,
    MembershipArchiveInspection, PreparedBackupControl,
};

const DIRECTORY: &str = "backup-generation-control";
const COMPLETE: &str = "generation-control-prepared.json";
const MAX_MANIFEST: usize = 4 * 1024 * 1024;

/// Actual original control bytes and normal native replay, not a successor
/// generation certificate, target membership or permission to release a fence.
#[derive(Clone, Debug)]
pub struct BackupGenerationControlReport {
    pub operation_id: OperationId,
    pub decision_hash: BackupRegistryDecisionHash,
    pub native_control_root: String,
    pub inputs_hash: String,
    pub targets: usize,
    pub original_members: usize,
    pub original_journal_frames: u64,
    pub original_journal_bytes_per_target: u64,
}
#[derive(Serialize)]
struct FileRef {
    bytes: usize,
    digest: String,
}
#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    decision_hash: &'a str,
    source_fence_hash: &'a str,
    generation_inputs_hash: &'a str,
    native_control_root: &'a str,
    files: BTreeMap<PathBuf, FileRef>,
}
#[derive(Serialize)]
struct Complete<'a> {
    format: &'static str,
    decision_hash: &'a str,
    inputs_hash: &'a str,
    native_control_root: &'a str,
    targets: usize,
}
struct Plan {
    archive: PreparedBackupControl,
    manifest: Vec<u8>,
    completion: Vec<u8>,
    inputs_hash: String,
}
fn bounded(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > MAX_MANIFEST {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "native generation control manifest exceeds 4 MiB",
        ));
    }
    Ok(bytes)
}
impl Plan {
    fn capture(
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        frozen: &BackupFrozenControl,
        generation: &BackupGenerationPreparationReport,
        index: usize,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let native = &prepared
            .task
            .installation
            .evidence
            .task
            .scoped
            .objects
            .native;
        let prefix = crate::engine::backup_preparation::native_control::archive::generation_objects::collect(
            &prepared.targets[index].root, &prepared.task.selection().jobs()[index],
            &native.archive_hash, stop,
        )?;
        let archive = PreparedBackupControl::capture(frozen, prefix, stop)?;
        let decision_hash = decision.hash();
        let manifest = bounded(&Manifest {
            format: "zxs-backup-generation-native-control",
            decision_hash: decision_hash.as_str(),
            source_fence_hash: prepared.source_fence_hash(),
            generation_inputs_hash: &generation.inputs_hash,
            native_control_root: archive.root_hash(),
            files: archive
                .files()
                .map(|(path, bytes)| {
                    (
                        PathBuf::from("archive").join(path),
                        FileRef {
                            bytes: bytes.len(),
                            digest: blake3::hash(bytes).to_hex().to_string(),
                        },
                    )
                })
                .collect(),
        })?;
        let inputs_hash = blake3::hash(&manifest).to_hex().to_string();
        let completion = bounded(&Complete {
            format: "zxs-backup-generation-native-control-prepared",
            decision_hash: decision_hash.as_str(),
            inputs_hash: &inputs_hash,
            native_control_root: archive.root_hash(),
            targets: prepared.targets.len(),
        })?;
        Ok(Self {
            archive,
            manifest,
            completion,
            inputs_hash,
        })
    }
    fn files(&self) -> Result<PreparedFiles<'_>> {
        PreparedFiles::new(
            Namespace::GenerationControl,
            self.manifest.clone(),
            self.archive
                .files()
                .map(|(path, bytes)| (PathBuf::from("archive").join(path), bytes)),
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
                "previous native control completion differs; no bytes replaced",
            ));
        }
        // Existing material-store reserve is 16 MiB; completion <=4 MiB.
        Ok(())
    }
    fn verify(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<MembershipArchiveInspection> {
        materials::verify(root, &self.files()?, stop)?;
        self.archive.verify(
            root.private_child(&root.path().join(DIRECTORY).join("archive"))?,
            stop,
        )
    }
    fn require_completion(&self, root: &DiskRoot) -> Result<()> {
        if root.read_bytes(&root.path().join(COMPLETE), self.completion.len())? != self.completion {
            return Err(invalid(
                "native fenced archive lacks its exact all-target completion",
            ));
        }
        Ok(())
    }
    fn report(
        &self,
        decision: &AdmittedBackupDecision,
        targets: usize,
        inspection: &MembershipArchiveInspection,
    ) -> BackupGenerationControlReport {
        BackupGenerationControlReport {
            operation_id: decision.operation(),
            decision_hash: decision.hash(),
            native_control_root: self.archive.root_hash().into(),
            inputs_hash: self.inputs_hash.clone(),
            targets,
            original_members: inspection.original_members,
            original_journal_frames: inspection.journal_frames,
            original_journal_bytes_per_target: inspection.journal_bytes,
        }
    }
}

pub(super) fn prepare(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    choices: &BackupChoiceContext,
    stop: &AtomicBool,
) -> Result<BackupGenerationControlReport> {
    recovery::require_isolated(&prepared.targets)?;
    let generation = generation_inputs::observe(prepared, decision, stop)?;
    let frozen = choices.freeze_generation_control(prepared, decision, stop)?;
    let first = Plan::capture(prepared, decision, &frozen, &generation, 0, stop)?;
    // Each target's own original prefix must yield the same complete archive.
    // No write begins until every target has passed this and its own capacity.
    for index in 0..prepared.targets.len() {
        preparation_checkpoint(stop)?;
        if index != 0 {
            let current = Plan::capture(prepared, decision, &frozen, &generation, index, stop)?;
            if current.manifest != first.manifest {
                return Err(invalid("target-native control prefixes disagree"));
            }
        }
        first.capacity(&prepared.targets[index].root, stop)?;
    }
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        recovery::require_isolated(&prepared.targets)?;
        prepared.recheck_names()?;
        materials::install(&target.root, &first.files()?, stop)?;
    }
    let mut inspected = Vec::with_capacity(prepared.targets.len());
    for target in &prepared.targets {
        inspected.push(first.verify(&target.root, stop)?);
    }
    choices.observe_decision(prepared, stop)?;
    // These bytes are diagnostics only. A crash between completion writes can
    // resume the same original task; never infer all-target success from one.
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
    choices.observe_decision(prepared, stop)?;
    recovery::require_isolated(&prepared.targets)?;
    Ok(first.report(decision, prepared.targets.len(), &inspected[0]))
}

pub(super) fn observe(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    stop: &AtomicBool,
) -> Result<BackupGenerationControlReport> {
    recovery::require_isolated(&prepared.targets)?;
    let generation = generation_inputs::observe(prepared, decision, stop)?;
    let mut report: Option<BackupGenerationControlReport> = None;
    for (index, target) in prepared.targets.iter().enumerate() {
        preparation_checkpoint(stop)?;
        let local = target
            .root
            .private_child(&target.root.path().join(DIRECTORY).join("archive"))?;
        let frozen = BackupFrozenControl::read_target(decision, &local, stop)?;
        let plan = Plan::capture(prepared, decision, &frozen, &generation, index, stop)?;
        let inspection = plan.verify(&target.root, stop)?;
        plan.require_completion(&target.root)?;
        if report
            .as_ref()
            .is_some_and(|first| first.inputs_hash != plan.inputs_hash)
        {
            return Err(invalid(
                "decided targets do not retain one exact original fenced control archive",
            ));
        }
        report = Some(plan.report(decision, prepared.targets.len(), &inspection));
    }
    decision.verify_targets(prepared, stop)?;
    recovery::require_isolated(&prepared.targets)?;
    prepared.recheck_names()?;
    Ok(report.expect("invariant: decided targets contains at least one complete target"))
}

/// Return this target's independently replayed native archive, not just its
/// stored root hash. The caller still owns the complete selected target set.
pub(super) fn read_authority_input(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    generation: &BackupGenerationPreparationReport,
    index: usize,
    stop: &AtomicBool,
) -> Result<(PreparedBackupControl, BackupGenerationControlReport)> {
    preparation_checkpoint(stop)?;
    let target = &prepared.targets[index];
    let local = target
        .root
        .private_child(&target.root.path().join(DIRECTORY).join("archive"))?;
    let frozen = BackupFrozenControl::read_target(decision, &local, stop)?;
    let plan = Plan::capture(prepared, decision, &frozen, generation, index, stop)?;
    let inspection = plan.verify(&target.root, stop)?;
    plan.require_completion(&target.root)?;
    let report = plan.report(decision, prepared.targets.len(), &inspection);
    Ok((plan.archive, report))
}
