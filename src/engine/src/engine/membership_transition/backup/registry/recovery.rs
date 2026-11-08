//! Read-only admission of an already selected common decision. A target's hash
//! is not an authority anchor: detached callers must supply an independently
//! retained digest. Live callers instead prove every actual original choice.
use super::*;

/// A caller-retained original native decision digest, NOT a value to discover
/// from the same untrusted target it is meant to authenticate. Parsing checks
/// representation only. Detached use grants frozen recovery reads, not writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupRegistryDecisionHash(String);
impl BackupRegistryDecisionHash {
    pub fn parse(value: &str) -> Result<Self> {
        require_hash(value)?;
        Ok(Self(value.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct StoredCore {
    format: String,
    transition_hash: String,
    transition: TransitionIntent,
    registry_placement: BackupRegistryProjection,
    original_members: BTreeMap<String, OriginalMember>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine::membership_transition) struct BackupDecisionCertificate {
    format: String,
    core_hash: String,
    core: StoredCore,
    decisions: BTreeMap<String, DecisionReceipt>,
}

/// This admits only the exact frozen choice. It cannot be converted to an
/// active Generation or used to clear membership_fence/blocked.
pub(in crate::engine) struct AdmittedBackupDecision {
    stored: BackupDecisionCertificate,
    core: PreparedMetadataObject,
    certificate: PreparedMetadataObject,
}
impl AdmittedBackupDecision {
    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_DECISION_BYTES {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "backup decision exceeds 32 MiB",
            ));
        }
        let stored: BackupDecisionCertificate = crate::journal::decode_proof(bytes)?;
        let admitted = Self::from_certificate(stored)?;
        if admitted.certificate.bytes() != bytes {
            return Err(invalid(
                "backup decision bytes are not the exact selected canonical object",
            ));
        }
        Ok(admitted)
    }
    pub(in crate::engine::membership_transition) fn from_certificate(
        stored: BackupDecisionCertificate,
    ) -> Result<Self> {
        if stored.format != CERTIFICATE_KIND || stored.core.format != CORE_KIND {
            return Err(invalid("unsupported backup registry decision"));
        }
        let core = PreparedMetadataObject::new(CORE_KIND, &stored.core, MAX_DECISION_BYTES)?;
        if stored.core_hash != core.hash {
            return Err(invalid("backup registry core digest differs"));
        }
        verify_intent(&stored.core.transition)?;
        if stored.core.transition_hash != object_hash(INTENT_KIND, &stored.core.transition)? {
            return Err(invalid("backup registry decision has another native fence"));
        }
        let Origin::BackupCutover { evidence, .. } = &stored.core.transition.origin else {
            unreachable!("invariant: verified Backup transition");
        };
        stored
            .core
            .registry_placement
            .verify_frozen_selection(evidence, &stored.core.transition_hash)?;
        verify_original_members(&stored)?;
        let certificate =
            PreparedMetadataObject::new(CERTIFICATE_KIND, &stored, MAX_DECISION_BYTES)?;
        Ok(Self {
            stored,
            core,
            certificate,
        })
    }
    pub(in crate::engine) fn from_trusted_bytes(
        bytes: &[u8],
        expected: &BackupRegistryDecisionHash,
    ) -> Result<Self> {
        let admitted = Self::decode(bytes)?;
        if admitted.certificate.hash != expected.as_str() {
            return Err(invalid(
                "target decision differs from independently retained digest",
            ));
        }
        Ok(admitted)
    }
    pub(in crate::engine) fn hash(&self) -> BackupRegistryDecisionHash {
        BackupRegistryDecisionHash(self.certificate.hash.clone())
    }
    /// Reserved native successor sequence only. Computing it creates neither
    /// a Candidate nor an operation result or publication proof.
    pub(in crate::engine) fn intended_first_sequence(&self) -> Result<u64> {
        self.stored
            .core
            .transition
            .fence_sequence
            .checked_add(1)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    "backup generation sequence exhausted",
                )
            })
    }
    pub(in crate::engine) fn operation(&self) -> OperationId {
        self.stored.core.transition.operation_id
    }
    pub(in crate::engine) fn verify_targets(
        &self,
        prepared: &PreparedBackupRegistry,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.bind_prepared(prepared)?;
        prepared.verify(stop)?;
        prepared.require_decision_copies(self.core.bytes(), self.certificate.bytes(), stop)
    }
    pub(in crate::engine) fn bind_source(
        &self,
        source_fence_hash: &str,
        source_evidence: &crate::engine::backup_preparation::BackupSourceFenceEvidence,
    ) -> Result<()> {
        let core = &self.stored.core;
        let Origin::BackupCutover { evidence, .. } = &core.transition.origin else {
            return Err(invalid(
                "decision is not the original backup source barrier",
            ));
        };
        if source_fence_hash != core.transition_hash
            || canonical_json(evidence.as_ref())? != canonical_json(source_evidence)?
        {
            return Err(invalid(
                "target installation belongs to another decided native source/task",
            ));
        }
        Ok(())
    }
    pub(in crate::engine::membership_transition) fn transition(&self) -> &TransitionIntent {
        &self.stored.core.transition
    }
    pub(in crate::engine::membership_transition) fn original_members(
        &self,
    ) -> &BTreeMap<String, OriginalMember> {
        &self.stored.core.original_members
    }
    pub(in crate::engine::membership_transition) fn projection(&self) -> &BackupRegistryProjection {
        &self.stored.core.registry_placement
    }
    pub(in crate::engine::membership_transition) fn certificate(
        &self,
    ) -> BackupDecisionCertificate {
        self.stored.clone()
    }
    fn bind_prepared(&self, prepared: &PreparedBackupRegistry) -> Result<()> {
        self.bind_source(prepared.source_fence_hash(), prepared.source_evidence())?;
        if canonical_json(prepared.projection())?
            != canonical_json(&self.stored.core.registry_placement)?
        {
            return Err(invalid(
                "complete topology or whole-unit placement differs from decided inputs",
            ));
        }
        Ok(())
    }
}

fn verify_original_members(certificate: &BackupDecisionCertificate) -> Result<()> {
    let core = &certificate.core;
    let original = core.transition.origin.config();
    if core.original_members.len() != original.metadata_members().len()
        || !core
            .original_members
            .keys()
            .eq(certificate.decisions.keys())
    {
        return Err(invalid(
            "decision must include every original member, not an online subset",
        ));
    }
    for disk in original.metadata_members() {
        let member = core
            .original_members
            .get(&disk.disk_id)
            .ok_or_else(|| invalid("decision omits an original member"))?;
        require_hash(&member.seal_hash)?;
        require_hash(&member.journal.content_hash)?;
        if member.identity
            != admission::member_identity(original, disk, member.identity.physical_identity())
            || member.journal.disk_id != disk.disk_id
            || member.journal.slot_id != disk.slot_id
            || member.journal.binding_generation != disk.binding_generation
            || member.journal.bytes == 0
            || member.journal.tail_frame_hash != member.fence.frame_hash
            || member.fence.disk_id != disk.disk_id
            || member.fence.slot_id != disk.slot_id
            || !receipt_binds(
                &member.fence,
                RecordType::MembershipFence,
                core.transition.fence_sequence,
                &core.transition_hash,
                &core.transition_hash,
            )?
        {
            return Err(invalid(
                "decision member is not its exact original identity/fence",
            ));
        }
        let acceptance = PreparedMetadataObject::new(
            ACCEPTANCE_KIND,
            &Acceptance {
                core_hash: &certificate.core_hash,
                original: member,
            },
            MAX_DECISION_BYTES,
        )?;
        certificate.decisions[&disk.disk_id].verify_backup(
            &core.transition_hash,
            &member.identity,
            &certificate.core_hash,
            &acceptance.hash,
        )?;
    }
    Ok(())
}
fn invalid(detail: &str) -> Error {
    Error::new(ErrorCode::PublicationInvalid, detail)
}
fn require_hash(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            "backup decision digest must be 64 lowercase hexadecimal characters",
        ));
    }
    Ok(())
}

impl BackupChoiceContext {
    fn retained_certificate(&self, prepared: &PreparedBackupRegistry) -> Result<Option<Vec<u8>>> {
        for source in self.sources.values() {
            let path = source.decision_path(&self.transition_hash);
            if preparation_path_exists(&source.root, &path)? {
                return source.root.read_bytes(&path, MAX_DECISION_BYTES).map(Some);
            }
        }
        prepared.decision_copy()
    }
    fn admit_actual_choices(
        &self,
        bytes: &[u8],
        prepared: &PreparedBackupRegistry,
        stop: &AtomicBool,
    ) -> Result<AdmittedBackupDecision> {
        let admitted = AdmittedBackupDecision::decode(bytes)?;
        admitted.bind_prepared(prepared)?;
        let core = &admitted.stored.core;
        if core.transition_hash != self.transition_hash
            || canonical_json(&core.transition)? != canonical_json(&self.intent)?
            || core.original_members.len() != self.sources.len()
        {
            return Err(invalid(
                "retained decision is not this original live source fence",
            ));
        }
        for (id, source) in &self.sources {
            preparation_checkpoint(stop)?;
            source.recheck(stop)?;
            if canonical_json(&core.original_members[id])? != canonical_json(&source.evidence)? {
                return Err(invalid(
                    "retained decision source seal/log/fence evidence differs",
                ));
            }
            // false never creates the namespace, neutral binding or a choice.
            let store = DecisionStore::open(
                &source.root,
                &source.evidence.identity,
                &self.transition_hash,
                false,
            )?;
            store.verify_receipt(&admitted.stored.decisions[id], false)?;
        }
        Ok(admitted)
    }
    /// Does not repair missing copies, select a slot, or trust the diagnostic
    /// phase. This is how an operator can retain an exact external anchor.
    pub(in crate::engine) fn observe_decision(
        &self,
        prepared: &PreparedBackupRegistry,
        stop: &AtomicBool,
    ) -> Result<AdmittedBackupDecision> {
        let bytes = self
            .retained_certificate(prepared)?
            .ok_or_else(|| invalid("no complete original native decision is retained"))?;
        let admitted = self.admit_actual_choices(&bytes, prepared, stop)?;
        admitted.verify_targets(prepared, stop)?;
        for source in self.sources.values() {
            source.recheck(stop)?;
            if source.root.read_bytes(
                &source.decision_path(&self.transition_hash),
                MAX_DECISION_BYTES,
            )? != bytes
            {
                return Err(invalid(
                    "original member lacks the exact complete decision copy",
                ));
            }
        }
        Ok(admitted)
    }
    /// Once any complete certificate exists, recovery cannot silently choose
    /// missing votes. ALL original slots must already hold its exact selection.
    /// Only absent copies of these already chosen bytes may be reinstalled.
    pub(in crate::engine) fn recover_decision_if_present(
        &self,
        prepared: &PreparedBackupRegistry,
        stop: &AtomicBool,
    ) -> Result<Option<String>> {
        let Some(bytes) = self.retained_certificate(prepared)? else {
            return Ok(None);
        };
        let admitted = self.admit_actual_choices(&bytes, prepared, stop)?;
        prepared.verify(stop)?;
        prepared.require_capacity(admitted.core.bytes(), admitted.certificate.bytes())?;
        for source in self.sources.values() {
            source.recheck(stop)?;
            source.require_capacity(
                &self.transition_hash,
                &admitted.core,
                &admitted.certificate,
            )?;
        }
        prepared.install_inputs(admitted.core.bytes(), stop)?;
        for (id, source) in &self.sources {
            preparation_checkpoint(stop)?;
            source.recheck(stop)?;
            // No call to select/backup_receipt on this recovery branch.
            let store = DecisionStore::open(
                &source.root,
                &source.evidence.identity,
                &self.transition_hash,
                false,
            )?;
            store.verify_receipt(&admitted.stored.decisions[id], true)?;
            admitted.core.store_at_root(&source.root, &source.disk)?;
            admitted
                .certificate
                .store_at_root(&source.root, &source.disk)?;
            install_preparation_bytes(
                &source.root,
                &source.decision_path(&self.transition_hash),
                &bytes,
            )?;
        }
        prepared.install_decision(&bytes, stop)?;
        // Re-enter the read-only gate instead of returning the repaired marker.
        Ok(Some(self.observe_decision(prepared, stop)?.hash().0))
    }
}
