//! Actual native singleton ACCEPT/FINALIZE/Publication under the stopped root's
//! explicit original lock. Publication is preserved, but not admitted to runtime.
use super::first::{Manifest, Prepared};
use super::*;
const SELECTED: &str = "chain-publication-selected.json";
const RECEIPT_FILE: &str = "chain-publication.json";
const JOURNAL: &str = "first-publication";
const TARGET_RECEIPT: &str = "published.json";
const MAX_EVIDENCE: usize = 128 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Recorded {
    manifest: Manifest,
    publication: Publication,
    seal: PublicationSeal,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TerminalChainFirstPublication {
    pub publication_hash: String,
    pub seal_hash: String,
    pub accept_frame_hash: String,
    pub finalize_frame_hash: String,
    pub publication_frame_hash: String,
    pub target_reverified_this_call: bool,
    pub active_current_admitted: bool,
}
pub(super) fn owns_initial_entry(name: &str) -> bool {
    matches!(name, SELECTED | RECEIPT_FILE)
}
pub(super) fn started(management: &TerminalManagement) -> Result<bool> {
    for name in [SELECTED, RECEIPT_FILE] {
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
            "terminal-chain Publication evidence lost its exact original first-input selection",
        ));
    }
    Ok(())
}
pub(super) fn require_not_started(management: &TerminalManagement) -> Result<()> {
    if started(management)? {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "continue the original terminal-chain Publication; do not reinstall first inputs",
        ));
    }
    Ok(())
}
fn selected_manifest(management: &TerminalManagement, prepared: &Prepared) -> Result<bool> {
    let saved: Option<Manifest> = load(
        management.system_root(),
        &operation(management).join(SELECTED),
    )?;
    match saved {
        Some(saved) => {
            activation::exact(&saved, prepared.manifest())?;
            Ok(true)
        }
        None => {
            require_absent(management)?;
            Ok(false)
        }
    }
}
impl Recorded {
    fn verify(&self, prepared: &Prepared) -> Result<()> {
        activation::exact(&self.manifest, prepared.manifest())?;
        verify_publication_seal(prepared.candidate(), &self.publication, &self.seal)?;
        let p = &self.publication;
        if p.recovery.is_some()
            || p.finalize.recovery_id.is_some()
            || !p.payload_receipts.is_empty()
            || canonical_json(&p.result)? != canonical_json(prepared.result())?
            || p.finalize.accepts.len() != 1
            || p.finalize_receipts.len() != 1
            || self.seal.receipts.len() != 1
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "terminal-chain Publication differs from its exact singleton first result",
            ));
        }
        let disk = prepared.disk();
        for r in p
            .finalize
            .accepts
            .iter()
            .chain(&p.finalize_receipts)
            .chain(&self.seal.receipts)
        {
            if r.disk_id != disk.disk_id || r.slot_id != disk.slot_id {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "terminal-chain Publication renamed the selected physical member's slot",
                ));
            }
        }
        if p.finalize.accepts[0].record.previous_frame_hash != "0".repeat(64) {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal-chain Publication is not the first original slot",
            ));
        }
        Ok(())
    }
    fn objects(&self) -> Result<[PreparedMetadataObject; 3]> {
        Ok([
            PreparedMetadataObject::new("finalize", &self.publication.finalize, MAX_EVIDENCE)?,
            PreparedMetadataObject::new("publication", &self.publication, MAX_EVIDENCE)?,
            PreparedMetadataObject::new("publication-seal", &self.seal, MAX_EVIDENCE)?,
        ])
    }
    fn verify_objects(&self, root: &DiskRoot) -> Result<()> {
        for object in self.objects()? {
            require_object(root, &object)?;
        }
        Ok(())
    }
    fn report(&self, target: bool) -> Result<TerminalChainFirstPublication> {
        Ok(TerminalChainFirstPublication {
            publication_hash: object_hash("publication", &self.publication)?,
            seal_hash: object_hash("publication-seal", &self.seal)?,
            accept_frame_hash: self.publication.finalize.accepts[0].frame_hash.clone(),
            finalize_frame_hash: self.publication.finalize_receipts[0].frame_hash.clone(),
            publication_frame_hash: self.seal.receipts[0].frame_hash.clone(),
            target_reverified_this_call: target,
            active_current_admitted: false,
        })
    }
}
fn require_object(root: &DiskRoot, object: &PreparedMetadataObject) -> Result<()> {
    let bytes = root.read_bytes(
        &root
            .path()
            .join(first::address(object.kind(), &object.hash)?),
        MAX_EVIDENCE,
    )?;
    if bytes != object.bytes() {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "terminal-chain actual Publication dependency missing or different",
        ));
    }
    Ok(())
}
fn store_object(root: &DiskRoot, object: &PreparedMetadataObject) -> Result<()> {
    let path = root
        .path()
        .join(first::address(object.kind(), &object.hash)?);
    root.private_dir(path.parent().expect("invariant: native object has parent"))?;
    root.write_atomic(&path, object.bytes(), false)?;
    require_object(root, object)
}
fn source_root(management: &TerminalManagement) -> Result<DiskRoot> {
    management
        .system_root()
        .private_child(&operation(management).join(activation::DIRECTORY))
}
pub(super) fn observe(
    management: &TerminalManagement,
    prepared: &Prepared,
    inputs_complete: bool,
    status: &mut TerminalChainReadmissionStatus,
) -> Result<()> {
    if !selected_manifest(management, prepared)? {
        return Ok(());
    }
    if !inputs_complete {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "terminal-chain Publication lost complete source inputs",
        ));
    }
    status.phase = TerminalChainReadmissionPhase::FirstPublicationPending;
    let recorded: Option<Recorded> = load(
        management.system_root(),
        &operation(management).join(RECEIPT_FILE),
    )?;
    if let Some(recorded) = recorded {
        recorded.verify(prepared)?;
        recorded.verify_objects(&source_root(management)?)?;
        status.phase = TerminalChainReadmissionPhase::FirstPublicationPreserved;
        status.publication = Some(recorded.report(false)?);
        status
            .activation
            .as_mut()
            .expect("invariant: complete native inputs were observed")
            .first_publication_recorded = true;
    }
    Ok(())
}

/// Constructible only by the original complete native publication path below.
/// The restricted journal gets a scoped JournalIo, never File/DiskIoBinding or
/// a generic unbound-root callback. Its lifetime borrows the original owners.
pub(crate) struct FirstSlotOwner<'a> {
    management: &'a TerminalManagement,
    selection: &'a Selection,
    held: &'a target::Held,
    prepared: &'a Prepared,
    source: DiskRoot,
    target: DiskRoot,
}
impl FirstSlotOwner<'_> {
    pub(crate) fn disk(&self) -> &DiskConfig {
        self.prepared.disk()
    }
    pub(crate) fn sequence(&self) -> u64 {
        self.prepared.manifest().sequence
    }
    pub(crate) fn candidate_hash(&self) -> &str {
        &self.prepared.manifest().candidate_hash
    }
    pub(crate) fn result(&self) -> &OperationResult {
        self.prepared.result()
    }
    pub(crate) fn recheck(&self) -> Result<()> {
        self.held.verify(&self.selection.proposal)?;
        require_selected(self.management, self.selection)?;
        if !selected_manifest(self.management, self.prepared)? {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "first-slot journal lost original source selection",
            ));
        }
        Ok(())
    }
    pub(crate) fn journal_io(&self, create: bool) -> Result<Option<crate::disk::JournalIo>> {
        self.recheck()?;
        let path = self.target.path().join(JOURNAL);
        if self.target.optional_metadata(&path)?.is_none() {
            if !create {
                return Ok(None);
            }
            self.target.private_dir(&path)?;
        }
        self.target.journal_directory(&path).map(Some)
    }
    fn evidence_before(
        &self,
        object: &PreparedMetadataObject,
        already_started: bool,
    ) -> Result<()> {
        self.recheck()?;
        for root in [&self.source, &self.target] {
            if already_started {
                require_object(root, object)?;
            }
            // Exact existing bytes may repeat their original durability fences;
            // a missing object after its journal frame started is never rebuilt.
            store_object(root, object)?;
        }
        Ok(())
    }
    fn verify_recorded(&self, recorded: &Recorded) -> Result<()> {
        self.read_recorded(recorded).map(|_| ())
    }
    fn read_recorded(&self, recorded: &Recorded) -> Result<crate::history_package::AdmittedState> {
        self.recheck()?;
        recorded.verify(self.prepared)?;
        recorded.verify_objects(&self.source)?;
        recorded.verify_objects(&self.target)?;
        self.prepared.verify_at(&self.source)?;
        let state = self.prepared.read_at(&self.target)?;
        self.held.verify_decision(self.selection)?;
        let observed = TerminalChainFirstJournal::inspect(self)?.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "recorded terminal-chain Publication lost target journal",
            )
        })?;
        let receipt = observed.complete(&object_hash("publication", &recorded.publication)?)?;
        if recorded.seal.receipts.as_slice() != std::slice::from_ref(&receipt) {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal-chain Publication seal differs from actual target journal",
            ));
        }
        self.recheck()?;
        Ok(state)
    }
}
impl TerminalManagement {
    /// Preserves an actual first-slot Publication. It does not consume Stop,
    /// admit Current/writer, start service or return a completed business result.
    pub fn publish_chain_readmission_first(
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
                "terminal-chain publication requires original source selection",
            )
        })?;
        if selection.proposal.principal() != principal
            || selection.proposal.operation() != operation_id
            || selection.proposal.hash()? != proposal_digest
            || selection.proposal.target().disk_id != directory.disk_id
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "terminal-chain publication must continue its original actor/operation/target",
            ));
        }
        let certificate = activation::certificate(self, &selection)?;
        let prepared = Prepared::new(&selection, &certificate, self.verified_authority())?;
        let inputs = activation::saved(self)?.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "terminal-chain publication requires original first inputs",
            )
        })?;
        activation::exact(&inputs, prepared.manifest())?;
        if !activation::source_prepared(self, &inputs)? {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "terminal-chain publication cannot precede complete two-root native inputs",
            ));
        }
        let source = source_root(self)?;
        prepared.verify_at(&source)?;
        let held = target::Held::open(self, &selection.proposal, directory)?;
        let (actual, independent_source) = held.verify_decision(&selection)?;
        let independent = Prepared::new(&selection, &actual, &independent_source)?;
        activation::exact(independent.manifest(), prepared.manifest())?;
        let target = held.root.private_child(
            &pending::member_directory(prepared.disk(), &selection.hash()?)
                .join(activation::TARGET_DIRECTORY),
        )?;
        let actual: Manifest = decode_strict(&target.read_bytes(
            &target.path().join(activation::TARGET_PREPARED),
            MAX_METADATA,
        )?)?;
        activation::exact(&actual, prepared.manifest())?;
        independent.verify_at(&target)?;
        let originally_selected = selected_manifest(self, &prepared)?;
        if !originally_selected
            && (target
                .optional_metadata(&target.path().join(JOURNAL))?
                .is_some()
                || target
                    .optional_metadata(&target.path().join(TARGET_RECEIPT))?
                    .is_some())
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "target first-publication evidence exists without original SYSTEM selection",
            ));
        }
        if !originally_selected {
            for root in [&source, &target] {
                if root.available()? < (MAX_EVIDENCE as u64 * 6 + 2 * 1024 * 1024) {
                    return Err(Error::new(
                        ErrorCode::NoSpace,
                        "each first-publication root needs independent journal/evidence capacity",
                    ));
                }
            }
        }
        require_selected(self, &selection)?;
        self.system_root().write_json(
            &operation(self).join(SELECTED),
            prepared.manifest(),
            false,
        )?;
        let owner = FirstSlotOwner {
            management: self,
            selection: &selection,
            held: &held,
            prepared: &prepared,
            source,
            target,
        };
        let source_record: Option<Recorded> =
            load(self.system_root(), &operation(self).join(RECEIPT_FILE))?;
        let target_record: Option<Recorded> =
            load(&owner.target, &owner.target.path().join(TARGET_RECEIPT))?;
        let recorded = match (source_record, target_record) {
            (Some(source), Some(target)) => {
                if canonical_json(&source)? != canonical_json(&target)? {
                    return Err(Error::new(
                        ErrorCode::LogFork,
                        "source and target first-publication records differ",
                    ));
                }
                owner.verify_recorded(&source)?;
                source
            }
            (Some(_), None) => {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "completed first Publication lost target record; do not rebuild",
                ));
            }
            (None, Some(target)) => {
                owner.verify_recorded(&target)?;
                target
            }
            (None, None) => publish(&owner)?,
        };
        owner.verify_recorded(&recorded)?;
        self.system_root()
            .write_json(&operation(self).join(RECEIPT_FILE), &recorded, false)?;
        let saved: Recorded = decode_strict(
            &self
                .system_root()
                .read_bytes(&operation(self).join(RECEIPT_FILE), MAX_EVIDENCE)?,
        )?;
        if canonical_json(&saved)? != canonical_json(&recorded)? {
            return Err(Error::new(
                ErrorCode::LogFork,
                "SYSTEM first Publication differs after durable receipt readback",
            ));
        }
        let mut status = self
            .chain_readmission_status()?
            .expect("invariant: selected native source remains owned");
        status.target_reverified_this_call = true;
        status
            .activation
            .as_mut()
            .expect("invariant: complete first inputs")
            .target_material_reverified_this_call = true;
        status
            .publication
            .as_mut()
            .expect("invariant: actual source Publication recorded")
            .target_reverified_this_call = true;
        Ok(status)
    }
}
fn publish(owner: &FirstSlotOwner<'_>) -> Result<Recorded> {
    let mut slot = TerminalChainFirstJournal::prepare(owner)?;
    let finalize_started = slot.stage_started(1)?;
    let publication_started = slot.stage_started(2)?;
    let accept = slot.accept()?;
    let finalize = Finalize {
        candidate_hash: owner.candidate_hash().into(),
        accepts: vec![accept],
        recovery_id: None,
    };
    let object = PreparedMetadataObject::new("finalize", &finalize, MAX_EVIDENCE)?;
    owner.evidence_before(&object, finalize_started)?;
    let finalized = slot.finalize(&finalize)?;
    let publication = Publication {
        candidate_hash: owner.candidate_hash().into(),
        finalize,
        finalize_receipts: vec![finalized],
        payload_receipts: Vec::new(),
        result: owner.result().clone(),
        recovery: None,
    };
    let object = PreparedMetadataObject::new("publication", &publication, MAX_EVIDENCE)?;
    owner.evidence_before(&object, publication_started)?;
    owner.prepared.verify_at(&owner.target)?;
    owner.prepared.verify_at(&owner.source)?;
    owner.held.verify_decision(owner.selection)?;
    let receipt = slot.publish(&publication)?;
    drop(slot);
    let observed = TerminalChainFirstJournal::inspect(owner)?.ok_or_else(|| {
        Error::new(
            ErrorCode::RecoveryRequired,
            "terminal-chain actual Publication disappeared after append",
        )
    })?;
    if observed.complete(&object.hash)? != receipt {
        return Err(Error::new(
            ErrorCode::LogFork,
            "terminal-chain Publication receipt differs after actual journal readback",
        ));
    }
    let recorded = Recorded {
        manifest: owner.prepared.manifest().clone(),
        seal: PublicationSeal {
            candidate_hash: owner.candidate_hash().into(),
            publication_hash: object.hash,
            receipts: vec![receipt],
        },
        publication,
    };
    recorded.verify(owner.prepared)?;
    let seal = PreparedMetadataObject::new("publication-seal", &recorded.seal, MAX_EVIDENCE)?;
    for root in [&owner.source, &owner.target] {
        store_object(root, &seal)?;
    }
    owner.verify_recorded(&recorded)?;
    PreparedMetadataObject::new("terminal-chain-publication-record", &recorded, MAX_EVIDENCE)?;
    owner
        .target
        .write_json(&owner.target.path().join(TARGET_RECEIPT), &recorded, false)?;
    let saved: Recorded = decode_strict(
        &owner
            .target
            .read_bytes(&owner.target.path().join(TARGET_RECEIPT), MAX_EVIDENCE)?,
    )?;
    if canonical_json(&saved)? != canonical_json(&recorded)? {
        return Err(Error::new(
            ErrorCode::LogFork,
            "target first Publication record differs after readback",
        ));
    }
    Ok(recorded)
}

/// Exact full native input and actual three-frame target readback. This owned
/// evidence admits a frozen namespace reader, never I/O or Engine installation.
/// Only `read_existing` below constructs it; persisted JSON is not this token.
pub(in crate::engine) struct PublishedProof {
    candidate: Candidate,
    recorded: Recorded,
    state: crate::history_package::AdmittedState,
    frozen_sequence: u64,
}
impl PublishedProof {
    pub(in crate::engine) fn state(&self) -> &State {
        self.state.state()
    }
    pub(in crate::engine) fn registry(&self) -> &crate::DiskRegistryCommitment {
        self.candidate
            .registry()
            .expect("invariant: exact chain candidate has a registry")
    }
    pub(in crate::engine) fn snapshot(&self) -> &str {
        &self.candidate.snapshot_hash
    }
    pub(in crate::engine) fn frozen_snapshot(&self) -> &str {
        self.candidate
            .previous_snapshot
            .as_deref()
            .expect("invariant: chain first slot has exact published predecessor")
    }
    pub(in crate::engine) fn frozen_sequence(&self) -> u64 {
        self.frozen_sequence
    }
    pub(super) fn report(&self) -> Result<TerminalChainFirstPublication> {
        self.recorded.report(true)
    }
    pub(super) fn manifest(&self) -> &Manifest {
        &self.recorded.manifest
    }
}

/// No installation, receipt completion, tail repair or source-only fallback.
/// Even a self-consistent source seal cannot replace the actual held target log.
pub(super) fn read_existing(
    management: &TerminalManagement,
    selection: &Selection,
    prepared: &Prepared,
    held: &target::Held,
) -> Result<PublishedProof> {
    let inputs = activation::saved(management)?.ok_or_else(|| {
        Error::new(
            ErrorCode::RecoveryRequired,
            "published Current requires original first inputs",
        )
    })?;
    activation::exact(&inputs, prepared.manifest())?;
    if !activation::source_prepared(management, &inputs)?
        || !selected_manifest(management, prepared)?
    {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "published Current requires completed original input and Publication selections",
        ));
    }
    let source_record: Recorded = load(
        management.system_root(),
        &operation(management).join(RECEIPT_FILE),
    )?
    .ok_or_else(|| {
        Error::new(
            ErrorCode::RecoveryRequired,
            "published Current requires completed SYSTEM Publication evidence",
        )
    })?;
    let target = held.root.private_child(
        &pending::member_directory(prepared.disk(), &selection.hash()?)
            .join(activation::TARGET_DIRECTORY),
    )?;
    let target_inputs: Manifest = decode_strict(&target.read_bytes(
        &target.path().join(activation::TARGET_PREPARED),
        MAX_METADATA,
    )?)?;
    activation::exact(&target_inputs, prepared.manifest())?;
    let target_record: Recorded =
        decode_strict(&target.read_bytes(&target.path().join(TARGET_RECEIPT), MAX_EVIDENCE)?)?;
    if canonical_json(&source_record)? != canonical_json(&target_record)? {
        return Err(Error::new(
            ErrorCode::LogFork,
            "published Current source and target Publication evidence differ",
        ));
    }
    let owner = FirstSlotOwner {
        management,
        selection,
        held,
        prepared,
        source: source_root(management)?,
        target,
    };
    let state = owner.read_recorded(&source_record)?;
    Ok(PublishedProof {
        candidate: prepared.candidate().clone(),
        recorded: source_record,
        state,
        frozen_sequence: selection
            .terminal
            .intent()
            .boundary
            .as_ref()
            .expect("invariant: chain source has exact published boundary")
            .sequence,
    })
}
