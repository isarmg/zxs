//! Native Backup authority objects in the isolated preparation domain. Every
//! target independently derives the same core from its actual native archive.
//! This is not generation.json, a first Publication or a writer installation.
use super::*;
use crate::disk::encode_metadata;
use crate::engine::backup_preparation::native_control::databases::materials::{
    self, Namespace, PreparedFiles,
};
use crate::engine::membership_transition::{
    AdmittedBackupDecision, BackupRegistryDecisionHash, PreparedBackupAuthority,
};
use crate::history_package::AdmittedState;

const DIRECTORY: &str = "backup-generation-authority";
const COMPLETE: &str = "generation-authority-prepared.json";
const MAX_MANIFEST: usize = 1024 * 1024;

/// Native-format selected identity only. A caller must not use this hash as a
/// published generation, a migration-success receipt or source-unfence proof.
#[derive(Clone, Debug)]
pub struct BackupGenerationAuthorityReport {
    pub operation_id: OperationId,
    pub decision_hash: BackupRegistryDecisionHash,
    pub authority_hash: String,
    pub native_control_root: String,
    pub routing_snapshot_hash: String,
    pub intended_first_sequence: u64,
    pub targets: usize,
    pub authority_bytes_per_target: u64,
}
#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    decision_hash: &'a str,
    source_fence_hash: &'a str,
    authority_hash: &'a str,
    native_control_root: &'a str,
    routing_snapshot_hash: &'a str,
    generation_inputs_hash: &'a str,
    control_inputs_hash: &'a str,
    object_path: &'a PathBuf,
    object_bytes: usize,
    object_digest: String,
}
#[derive(Serialize)]
struct Completion<'a> {
    format: &'static str,
    decision_hash: &'a str,
    authority_hash: &'a str,
    manifest_hash: &'a str,
    targets: usize,
}
struct Plan {
    authority: PreparedBackupAuthority,
    object_path: PathBuf,
    manifest: Vec<u8>,
    completion: materials::RootRecord<'static>,
    report: BackupGenerationAuthorityReport,
}
impl Plan {
    fn capture(
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        inputs: &BackupGenerationPreparationReport,
        index: usize,
        stop: &AtomicBool,
    ) -> Result<Self> {
        Self::capture_input(prepared, decision, inputs, index, stop).map(|(plan, _)| plan)
    }
    fn capture_input(
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        inputs: &BackupGenerationPreparationReport,
        index: usize,
        stop: &AtomicBool,
    ) -> Result<(Self, AdmittedState)> {
        let (control, observed) =
            generation_control::read_authority_input(prepared, decision, inputs, index, stop)?;
        let source = snapshot::read(
            &prepared.targets[index].root,
            &prepared.task.selection().jobs()[index],
            stop,
        )?;
        let authority = PreparedBackupAuthority::capture(
            decision,
            &control,
            inputs,
            &observed,
            source.state(),
        )?;
        let decision_hash = decision.hash();
        let object_path = PathBuf::from("objects")
            .join("membership-generation")
            .join(&authority.hash()[..2])
            .join(format!("{}.json", authority.hash()));
        let manifest = encode_metadata(
            &Manifest {
                format: "zxs-backup-native-authority-inputs",
                decision_hash: decision_hash.as_str(),
                source_fence_hash: prepared.source_fence_hash(),
                authority_hash: authority.hash(),
                native_control_root: &observed.native_control_root,
                routing_snapshot_hash: &inputs.routing_snapshot_hash,
                generation_inputs_hash: &inputs.inputs_hash,
                control_inputs_hash: &observed.inputs_hash,
                object_path: &object_path,
                object_bytes: authority.bytes().len(),
                object_digest: blake3::hash(authority.bytes()).to_hex().to_string(),
            },
            MAX_MANIFEST,
        )?;
        let manifest_hash = blake3::hash(&manifest).to_hex().to_string();
        let completion = encode_metadata(
            &Completion {
                format: "zxs-backup-native-authority-prepared",
                decision_hash: decision_hash.as_str(),
                authority_hash: authority.hash(),
                manifest_hash: &manifest_hash,
                targets: prepared.targets.len(),
            },
            MAX_MANIFEST,
        )?;
        let report = BackupGenerationAuthorityReport {
            operation_id: decision.operation(),
            decision_hash,
            authority_hash: authority.hash().into(),
            native_control_root: observed.native_control_root,
            routing_snapshot_hash: inputs.routing_snapshot_hash.clone(),
            intended_first_sequence: inputs.intended_first_sequence,
            targets: prepared.targets.len(),
            authority_bytes_per_target: authority.bytes().len() as u64,
        };
        Ok((
            Self {
                authority,
                object_path,
                manifest,
                completion: materials::RootRecord::owned(COMPLETE, completion.len(), completion),
                report,
            },
            source,
        ))
    }
    fn files(&self) -> Result<PreparedFiles<'_>> {
        PreparedFiles::new(
            Namespace::GenerationAuthority,
            self.manifest.as_slice(),
            [(self.object_path.clone(), self.authority.bytes())],
        )
    }
    fn capacity(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        materials::require_capacity(root, &self.files()?, stop)?;
        self.completion.check_existing(root)?;
        // The material store's 16 MiB reserve also covers this <=1 MiB marker.
        Ok(())
    }
    fn verify(&self, root: &DiskRoot, stop: &AtomicBool) -> Result<()> {
        materials::verify(root, &self.files()?, stop)?;
        let bytes = root.read_bytes(
            &root.path().join(DIRECTORY).join(&self.object_path),
            self.authority.bytes().len(),
        )?;
        self.authority.verify_saved(&bytes)
    }
}

pub(super) fn prepare(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    stop: &AtomicBool,
) -> Result<BackupGenerationAuthorityReport> {
    recovery::require_isolated(&prepared.targets)?;
    decision.verify_targets(prepared, stop)?;
    let inputs = generation_inputs::observe(prepared, decision, stop)?;
    let first = Plan::capture(prepared, decision, &inputs, 0, stop)?;
    // No new write until every independent target has its actual archive and
    // yields the same canonical authority and capacity; never borrow a peer.
    for index in 0..prepared.targets.len() {
        preparation_checkpoint(stop)?;
        if index != 0 {
            let next = Plan::capture(prepared, decision, &inputs, index, stop)?;
            if next.manifest != first.manifest {
                return Err(invalid("selected target Backup authority cores disagree"));
            }
        }
        first.capacity(&prepared.targets[index].root, stop)?;
    }
    prepared.write_isolated_targets(stop, |_, target| {
        materials::install(&target.root, &first.files()?, stop)
    })?;
    for target in &prepared.targets {
        first.verify(&target.root, stop)?;
    }
    prepared.write_completions(&first.completion, stop)?;
    decision.verify_targets(prepared, stop)?;
    recovery::require_isolated(&prepared.targets)?;
    Ok(first.report)
}

pub(super) fn observe(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    stop: &AtomicBool,
) -> Result<BackupGenerationAuthorityReport> {
    recovery::require_isolated(&prepared.targets)?;
    decision.verify_targets(prepared, stop)?;
    let inputs = generation_inputs::observe(prepared, decision, stop)?;
    let mut first: Option<BackupGenerationAuthorityReport> = None;
    for index in 0..prepared.targets.len() {
        preparation_checkpoint(stop)?;
        let plan = Plan::capture(prepared, decision, &inputs, index, stop)?;
        plan.verify(&prepared.targets[index].root, stop)?;
        plan.completion.require(&prepared.targets[index].root)?;
        if first
            .as_ref()
            .is_some_and(|first| first.authority_hash != plan.report.authority_hash)
        {
            return Err(invalid(
                "Backup authority recovery does not bind one complete original batch",
            ));
        }
        first = Some(plan.report);
    }
    decision.verify_targets(prepared, stop)?;
    recovery::require_isolated(&prepared.targets)?;
    Ok(first.expect("invariant: admitted preparation contains one to eight targets"))
}

/// Borrow no source/peer shortcut: the first-candidate recipe receives this
/// target's own complete admitted State and its read-back native authority.
pub(super) fn read_candidate_input(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    inputs: &BackupGenerationPreparationReport,
    index: usize,
    stop: &AtomicBool,
) -> Result<(PreparedBackupAuthority, AdmittedState)> {
    let (plan, source) = Plan::capture_input(prepared, decision, inputs, index, stop)?;
    plan.verify(&prepared.targets[index].root, stop)?;
    plan.completion.require(&prepared.targets[index].root)?;
    Ok((plan.authority, source))
}
