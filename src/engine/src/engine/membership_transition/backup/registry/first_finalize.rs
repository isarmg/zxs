//! Real first-slot ACCEPT/FINALIZE on ALL original metadata disks. Neither
//! target material nor a planned receipt can select this slot. No Publication,
//! marker, serving seal, active Config or source-unfence is produced here.
use super::*;
use crate::engine::membership_transition::PreparedBackupFirstCandidate;
use crate::journal::BackupFirstJournal;

#[derive(Clone, Debug)]
pub struct BackupFirstFinalizeReport {
    pub operation_id: OperationId,
    pub decision_hash: BackupRegistryDecisionHash,
    pub authority_hash: String,
    pub candidate_hash: String,
    pub snapshot_hash: String,
    pub finalize_hash: String,
    pub first_sequence: u64,
    pub original_metadata_members: Vec<String>,
}

fn incomplete(detail: &str) -> Error {
    Error::new(ErrorCode::RecoveryRequired, detail)
}
fn finalization(
    candidate: &PreparedBackupFirstCandidate,
    accepts: Vec<Receipt>,
) -> Result<Finalize> {
    let original = candidate.original_config();
    let mut actual = BTreeMap::new();
    for receipt in &accepts {
        let disk = original
            .metadata_members()
            .find(|disk| disk.disk_id == receipt.disk_id)
            .ok_or_else(|| incomplete("first Backup ACCEPT names a non-original member"))?;
        if disk.slot_id != receipt.slot_id
            || actual.insert(receipt.disk_id.as_str(), ()).is_some()
            || receipt.record.previous_frame_hash != "0".repeat(64)
            || !receipt_binds(
                receipt,
                RecordType::Accept,
                candidate.sequence(),
                candidate.hash(),
                candidate.hash(),
            )?
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "first Backup ACCEPT does not bind one actual original first slot",
            ));
        }
    }
    if actual.len() != original.metadata_members().len() {
        return Err(incomplete(
            "first Backup FINALIZE requires ALL original members, never an online subset",
        ));
    }
    Ok(Finalize {
        candidate_hash: candidate.hash().into(),
        accepts,
        recovery_id: None,
    })
}
fn report(
    candidate: &PreparedBackupFirstCandidate,
    finalize_hash: String,
    operation_id: OperationId,
) -> BackupFirstFinalizeReport {
    BackupFirstFinalizeReport {
        operation_id,
        decision_hash: candidate.decision_hash().clone(),
        authority_hash: candidate.authority_hash().into(),
        candidate_hash: candidate.hash().into(),
        snapshot_hash: candidate.snapshot_hash().into(),
        finalize_hash,
        first_sequence: candidate.sequence(),
        original_metadata_members: candidate.members().to_vec(),
    }
}

impl BackupChoiceContext {
    pub(super) fn require_first_candidate(
        &self,
        prepared: &PreparedBackupRegistry,
        candidate: &PreparedBackupFirstCandidate,
        stop: &AtomicBool,
    ) -> Result<()> {
        let decision = self.observe_decision(prepared, stop)?;
        if decision.hash() != *candidate.decision_hash()
            || decision.intended_first_sequence()? != candidate.sequence()
            || canonical_json(self.intent.origin.config())?
                != canonical_json(candidate.original_config())?
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "first Backup candidate belongs to another actual original decision",
            ));
        }
        Ok(())
    }

    /// Called only after every isolated target independently read back the exact
    /// typed candidate and complete image. The source locks/leases remain owned
    /// by this task. All native writes use the original held disk bindings.
    pub(in crate::engine) fn finalize_first_candidate(
        &self,
        prepared: &PreparedBackupRegistry,
        candidate: &PreparedBackupFirstCandidate,
        stop: &AtomicBool,
    ) -> Result<BackupFirstFinalizeReport> {
        self.require_first_candidate(prepared, candidate, stop)?;
        let mut slots = BTreeMap::new();
        let mut has_finalize = false;
        let mut publication_started = false;
        // No new journal or vote before every original prefix and capacity is
        // checked. Unknown/future records are never truncated into this format.
        for (id, source) in &self.sources {
            source.recheck(stop)?;
            candidate.require_member_capacity(&source.root)?;
            let slot = BackupFirstJournal::inspect(&source.root, &source.disk, candidate)?;
            if let Some(slot) = &slot {
                has_finalize |= slot.finalized_hash()?.is_some();
                publication_started |= slot.publication_started()?;
            }
            slots.insert(id.clone(), slot);
        }
        if publication_started {
            // A third frame, even torn, is after the ALL-FINALIZE barrier. A
            // missing earlier receipt/object is not a fresh preparation job.
            return self.observe_first_finalize(prepared, candidate, stop);
        }
        if has_finalize {
            // A complete FINALIZE proves the earlier all-ACCEPT barrier was
            // reached. Loss of a required ACCEPT is corruption, not permission
            // to replace the member's missing vote with a planned frame.
            let accepts = slots
                .values()
                .map(|slot| {
                    slot.as_ref()
                        .ok_or_else(|| {
                            incomplete("original ACCEPT missing after another member FINALIZE")
                        })?
                        .accepted()?
                        .ok_or_else(|| {
                            incomplete("original ACCEPT missing after another member FINALIZE")
                        })
                })
                .collect::<Result<Vec<_>>>()?;
            let expected = object_hash("finalize", &finalization(candidate, accepts)?)?;
            for slot in slots.values().flatten() {
                if slot.finalized_hash()?.is_some_and(|hash| hash != expected) {
                    return Err(Error::new(
                        ErrorCode::LogFork,
                        "original members have different Backup FINALIZE vectors",
                    ));
                }
            }
        }
        // Immutable native metadata is on every original voter, not only on
        // the new data targets, before any new ACCEPT can be acknowledged.
        for source in self.sources.values() {
            source.recheck(stop)?;
            candidate.install_on_original(&source.root, &source.disk, stop)?;
        }
        self.require_first_candidate(prepared, candidate, stop)?;
        let mut accepts = Vec::with_capacity(self.sources.len());
        for (id, source) in &self.sources {
            preparation_checkpoint(stop)?;
            source.recheck(stop)?;
            prepared.recheck_names()?;
            let slot = slots
                .get_mut(id)
                .expect("invariant: all original members preflighted");
            if slot.is_none() {
                *slot = Some(BackupFirstJournal::prepare(
                    &source.root,
                    &source.disk,
                    candidate,
                )?);
            }
            let receipt = slot
                .as_mut()
                .expect("invariant: exact source slot opened")
                .accept()?;
            let observed = BackupFirstJournal::inspect(&source.root, &source.disk, candidate)?
                .ok_or_else(|| incomplete("first Backup journal disappeared after ACCEPT"))?;
            if observed.accepted()?.as_ref() != Some(&receipt) {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "actual Backup ACCEPT differs after synchronization",
                ));
            }
            accepts.push(receipt);
        }
        // Construct the native Finalize from receipts returned by real writes
        // and read again from their actual disks. No planned vector is used.
        self.require_first_candidate(prepared, candidate, stop)?;
        let finalize = finalization(candidate, accepts)?;
        let object = PreparedMetadataObject::new("finalize", &finalize, MAX_METADATA)?;
        for slot in slots.values().flatten() {
            if slot
                .finalized_hash()?
                .is_some_and(|hash| hash != object.hash)
            {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "Backup FINALIZE conflicts before any append",
                ));
            }
        }
        for source in self.sources.values() {
            source.recheck(stop)?;
            object.store_at_root(&source.root, &source.disk)?;
        }
        for (id, source) in &self.sources {
            preparation_checkpoint(stop)?;
            source.recheck(stop)?;
            prepared.recheck_names()?;
            slots
                .get_mut(id)
                .and_then(Option::as_mut)
                .expect("invariant: actual ACCEPT owns original journal")
                .finalize(&finalize)?;
        }
        drop(slots);
        self.observe_first_finalize(prepared, candidate, stop)
    }

    /// Only current physical source journals prove this live stage. It does not
    /// accept a target-side copy as an original member's acknowledgement, repair
    /// a torn suffix, issue a vote, or infer success from SYSTEM progress.
    pub(in crate::engine) fn observe_first_finalize(
        &self,
        prepared: &PreparedBackupRegistry,
        candidate: &PreparedBackupFirstCandidate,
        stop: &AtomicBool,
    ) -> Result<BackupFirstFinalizeReport> {
        let (finalize, _) = self.first_finalize_evidence(prepared, candidate, stop)?;
        Ok(report(
            candidate,
            object_hash("finalize", &finalize)?,
            self.intent.operation_id,
        ))
    }

    pub(super) fn first_finalize_evidence(
        &self,
        prepared: &PreparedBackupRegistry,
        candidate: &PreparedBackupFirstCandidate,
        stop: &AtomicBool,
    ) -> Result<(Finalize, Vec<Receipt>)> {
        self.require_first_candidate(prepared, candidate, stop)?;
        let mut slots = BTreeMap::new();
        let mut accepts = Vec::with_capacity(self.sources.len());
        for (id, source) in &self.sources {
            source.recheck(stop)?;
            candidate.verify_on_original(&source.root, &source.disk, stop)?;
            let slot = BackupFirstJournal::inspect(&source.root, &source.disk, candidate)?
                .ok_or_else(|| incomplete("original first Backup journal is absent"))?;
            accepts.push(
                slot.accepted()?
                    .ok_or_else(|| incomplete("original first Backup ACCEPT is absent"))?,
            );
            slots.insert(id.clone(), slot);
        }
        let finalize = finalization(candidate, accepts)?;
        let object = PreparedMetadataObject::new("finalize", &finalize, MAX_METADATA)?;
        let mut receipts = Vec::with_capacity(self.sources.len());
        for (id, source) in &self.sources {
            source.recheck(stop)?;
            let actual =
                load_authenticated_object_at(&source.root, &source.disk, "finalize", &object.hash)?;
            if actual.bytes() != object.bytes() {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "original native Finalize object differs",
                ));
            }
            receipts.push(slots[id].require_finalized(&object.hash)?);
        }
        self.require_first_candidate(prepared, candidate, stop)?;
        Ok((finalize, receipts))
    }
}
