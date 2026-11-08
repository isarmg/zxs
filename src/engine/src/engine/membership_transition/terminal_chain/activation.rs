//! Immutable selection and two-root installation of native first-slot inputs.
//! Neither root's prepared marker is Publication, an active writer or a locator.
use super::first::{Manifest, Prepared};
use super::*;
const SELECTED: &str = "chain-first-selected.json";
const PREPARED: &str = "chain-first-prepared.json";
const DIRECTORY: &str = "chain-first";
const TARGET_DIRECTORY: &str = "first-candidate";
const TARGET_PREPARED: &str = "prepared.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TerminalChainReadmissionActivation {
    pub core_hash: String,
    pub candidate_hash: String,
    pub snapshot_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub topology_generation: u64,
    pub native_objects: usize,
    #[serialization(with = "storage_contract::u64_string")]
    pub native_bytes: u64,
    pub source_material_reverified_this_call: bool,
    pub target_material_reverified_this_call: bool,
    pub first_publication_recorded: bool,
}
impl Manifest {
    fn report(&self, source: bool, target: bool) -> TerminalChainReadmissionActivation {
        TerminalChainReadmissionActivation {
            core_hash: self.core_hash.clone(),
            candidate_hash: self.candidate_hash.clone(),
            snapshot_hash: self.snapshot_hash.clone(),
            sequence: self.sequence,
            topology_generation: self.topology_generation,
            native_objects: self.native_objects,
            native_bytes: self.native_bytes,
            source_material_reverified_this_call: source,
            target_material_reverified_this_call: target,
            first_publication_recorded: false,
        }
    }
}
pub(super) fn owns_initial_entry(name: &str) -> bool {
    matches!(name, SELECTED | PREPARED | DIRECTORY)
}
pub(super) fn started(management: &TerminalManagement) -> Result<bool> {
    for name in [SELECTED, PREPARED, DIRECTORY] {
        if management
            .system_root()
            .optional_metadata(&operation(management).join(name))?
            .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(super) fn require_absent(management: &TerminalManagement) -> Result<()> {
    if started(management)? {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "terminal-chain first-slot material has no original selected request",
        ));
    }
    Ok(())
}
fn certificate(management: &TerminalManagement, selection: &Selection) -> Result<Certificate> {
    let certificate = load(
        management.system_root(),
        &operation(management).join(RECEIPT),
    )?
    .ok_or_else(|| {
        Error::new(
            ErrorCode::RecoveryRequired,
            "terminal-chain first slot requires the actual original target decision",
        )
    })?;
    selection.verify_certificate(&certificate)?;
    Ok(certificate)
}
fn exact(actual: &Manifest, expected: &Manifest) -> Result<()> {
    if actual != expected {
        return Err(Error::new(
            ErrorCode::LogFork,
            "terminal-chain first-slot selection differs from the complete original native recipe",
        ));
    }
    Ok(())
}
fn saved(management: &TerminalManagement) -> Result<Option<Manifest>> {
    let root = management.system_root();
    let selected = load(root, &operation(management).join(SELECTED))?;
    if selected.is_none() {
        require_absent(management)?;
    }
    Ok(selected)
}
fn source_prepared(management: &TerminalManagement, expected: &Manifest) -> Result<bool> {
    let observed: Option<Manifest> = load(
        management.system_root(),
        &operation(management).join(PREPARED),
    )?;
    if let Some(observed) = observed {
        exact(&observed, expected)?;
        return Ok(true);
    }
    Ok(false)
}
pub(super) fn observe(
    management: &TerminalManagement,
    selection: &Selection,
    status: &mut TerminalChainReadmissionStatus,
) -> Result<()> {
    let Some(manifest) = saved(management)? else {
        return Ok(());
    };
    let certificate = certificate(management, selection)?;
    let prepared = Prepared::new(selection, &certificate, management.verified_authority())?;
    exact(&manifest, prepared.manifest())?;
    let complete = source_prepared(management, &manifest)?;
    if complete {
        prepared.verify_at(
            &management
                .system_root()
                .private_child(&operation(management).join(DIRECTORY))?,
        )?;
    }
    status.phase = if complete {
        TerminalChainReadmissionPhase::FirstCandidatePrepared
    } else {
        TerminalChainReadmissionPhase::ActivationSelected
    };
    status.activation = Some(manifest.report(complete, false));
    Ok(())
}
pub(super) fn validate_source(
    management: &TerminalManagement,
    selection: &Selection,
) -> Result<()> {
    let mut status = selection.status(None, false)?;
    observe(management, selection, &mut status)
}

impl TerminalManagement {
    /// Writes only isolated native first-slot inputs for the original retained
    /// target. Failure preserves its selection; this never starts publication.
    pub fn prepare_chain_readmission_activation(
        &mut self,
        principal: &str,
        operation_id: OperationId,
        proposal_digest: &str,
        directory: PendingMemberDirectory,
    ) -> Result<TerminalChainReadmissionStatus> {
        Engine::require_actor(principal)?;
        let selection = selected(self)?.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "prepare the original target decision before terminal-chain first-slot inputs",
            )
        })?;
        if selection.proposal.principal() != principal
            || selection.proposal.operation() != operation_id
            || selection.proposal.hash()? != proposal_digest
            || selection.proposal.target().disk_id != directory.disk_id
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "terminal-chain first-slot request differs from its original actor/operation/target",
            ));
        }
        let original = certificate(self, &selection)?;
        let prepared = Prepared::new(&selection, &original, self.verified_authority())?;
        let expected = prepared.manifest();
        if let Some(saved) = saved(self)? {
            exact(&saved, expected)?;
        }
        let complete = source_prepared(self, expected)?;
        let held = target::Held::open(self, &selection.proposal, directory)?;
        let (actual, source) = held.verify_decision(&selection)?;
        if actual.decision != original.decision {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal-chain first-slot target differs from source's actual decision certificate",
            ));
        }
        // Rebuild from the target's own complete control archive and graph.
        // An identical manifest alone cannot bless omitted State or history pages.
        let target_prepared = Prepared::new(&selection, &actual, &source)?;
        exact(target_prepared.manifest(), expected)?;
        if !complete {
            prepared.require_capacity(self.system_root())?;
            target_prepared.require_capacity(&held.root)?;
        }
        require_selected(self, &selection)?;
        // Persist the exact recipe before creating either bundle. A retry may
        // only complete these bytes, not replace the target or source recipe.
        self.system_root()
            .write_json(&operation(self).join(SELECTED), expected, false)?;
        let target_path =
            pending::member_directory(selection.proposal.target_disk(), &selection.hash()?)
                .join(TARGET_DIRECTORY);
        let existing: Option<Manifest> = if held.root.optional_metadata(&target_path)?.is_some() {
            let root = held.root.private_child(&target_path)?;
            load(&root, &root.path().join(TARGET_PREPARED))?
        } else {
            None
        };
        if let Some(existing) = &existing {
            exact(existing, expected)?;
        }
        if complete && existing.is_none() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "completed terminal-chain first inputs lost their original target marker; do not recreate",
            ));
        }
        if existing.is_none() {
            held.root.private_dir(&target_path)?;
        }
        let target_root = held.root.private_child(&target_path)?;
        if existing.is_some() {
            target_prepared.verify_at(&target_root)?;
        } else {
            target_prepared.install(&target_root)?;
        }
        held.verify_decision(&selection)?;
        require_selected(self, &selection)?;
        target_root.write_json(&target_root.path().join(TARGET_PREPARED), expected, false)?;
        let target_marker: Manifest = decode_strict(
            &target_root.read_bytes(&target_root.path().join(TARGET_PREPARED), MAX_METADATA)?,
        )?;
        exact(&target_marker, expected)?;
        let source_path = operation(self).join(DIRECTORY);
        if !complete {
            self.system_root().private_dir(&source_path)?;
        }
        let source_root = self.system_root().private_child(&source_path)?;
        if complete {
            prepared.verify_at(&source_root)?;
        } else {
            prepared.install(&source_root)?;
        }
        // Final target reread precedes the immutable SYSTEM prepared receipt.
        // It does not admit the prospective PUBLISHED receipt to live State.
        target_prepared.verify_at(&target_root)?;
        held.verify_decision(&selection)?;
        require_selected(self, &selection)?;
        self.system_root()
            .write_json(&operation(self).join(PREPARED), expected, false)?;
        let source_marker: Manifest = decode_strict(
            &self
                .system_root()
                .read_bytes(&operation(self).join(PREPARED), MAX_METADATA)?,
        )?;
        exact(&source_marker, expected)?;
        let mut status = selection.status(Some(&original), true)?;
        status.phase = TerminalChainReadmissionPhase::FirstCandidatePrepared;
        status.activation = Some(expected.report(true, true));
        Ok(status)
    }
}
