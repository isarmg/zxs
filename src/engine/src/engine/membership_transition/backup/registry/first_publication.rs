//! A real first Publication in the selected, reserved native generation.
//! It protects bytes and the complete candidate through actual original votes;
//! it is not active Config/Current admission or permission to lift the fence.
use super::*;
use crate::engine::membership_transition::PreparedBackupFirstCandidate;
use crate::journal::BackupFirstJournal;
use payloads::{PayloadPlan, VerifiedPayloadReceipts};

mod evidence;
pub use evidence::BackupFirstPublicationHash;
pub(in crate::engine) use evidence::PreparedBackupFirstPublication;

#[derive(Clone, Debug)]
pub struct BackupFirstPublicationReport {
    pub operation_id: OperationId,
    pub decision_hash: BackupRegistryDecisionHash,
    pub authority_hash: String,
    pub candidate_hash: String,
    pub snapshot_hash: String,
    pub publication_hash: String,
    pub first_sequence: u64,
    pub original_metadata_members: Vec<String>,
    pub payload_receipts: usize,
}

fn publication(
    candidate: &PreparedBackupFirstCandidate,
    finalize: Finalize,
    finalize_receipts: Vec<Receipt>,
    payloads: VerifiedPayloadReceipts,
) -> Publication {
    Publication {
        candidate_hash: candidate.hash().into(),
        finalize,
        finalize_receipts,
        payload_receipts: payloads.into_receipts(),
        result: candidate.result().clone(),
        recovery: None,
    }
}
fn missing(detail: &str) -> Error {
    Error::new(ErrorCode::RecoveryRequired, detail)
}

impl BackupChoiceContext {
    /// Only the original fenced task reaches this path. Targets supply real
    /// payload acknowledgements, never ACCEPT/FINALIZE/Publication votes.
    pub(in crate::engine) fn publish_first_candidate(
        &self,
        prepared: &PreparedBackupRegistry,
        candidate: &PreparedBackupFirstCandidate,
        stop: &AtomicBool,
    ) -> Result<PreparedBackupFirstPublication> {
        let (finalize, finals) = self.first_finalize_evidence(prepared, candidate, stop)?;
        let mut slots = BTreeMap::new();
        let mut started = false;
        for (id, source) in &self.sources {
            source.recheck(stop)?;
            let slot = BackupFirstJournal::inspect(&source.root, &source.disk, candidate)?
                .ok_or_else(|| missing("original first Backup journal vanished after FINALIZE"))?;
            started |= slot.publication_started()?;
            slots.insert(id.clone(), slot);
        }
        let payloads = PayloadPlan::new(self, prepared, candidate, &finalize)?;
        payloads.require_capacity(self, &finals)?;
        // Once a complete or torn Publication exists, all earlier payload and
        // Finalize barriers must already remain. Never recreate a missing vote
        // or CommitProof merely because another disk retained a later receipt.
        let receipts = if started {
            payloads.observe(stop)?
        } else {
            payloads.install(stop)?
        };
        self.require_first_candidate(prepared, candidate, stop)?;
        let published = publication(candidate, finalize.clone(), finals, receipts);
        let object = PreparedMetadataObject::new("publication", &published, MAX_METADATA)?;
        for (id, source) in &self.sources {
            source.recheck(stop)?;
            if slots[id]
                .published_hash()?
                .is_some_and(|hash| hash != object.hash)
            {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "original Backup member published a different first result",
                ));
            }
            if started {
                let saved = load_authenticated_object_at(
                    &source.root,
                    &source.disk,
                    "publication",
                    &object.hash,
                )?;
                if saved.bytes() != object.bytes() {
                    return Err(Error::new(
                        ErrorCode::PublicationInvalid,
                        "first Backup Publication lost its original complete object",
                    ));
                }
            }
        }
        // The exact immutable object is durable on ALL original members before
        // a first Publication frame can exist. A target's completion marker is
        // not accepted as either this object or an original member's receipt.
        for source in self.sources.values() {
            preparation_checkpoint(stop)?;
            source.recheck(stop)?;
            object.store_at_root(&source.root, &source.disk)?;
            let actual = load_authenticated_object_at(
                &source.root,
                &source.disk,
                "publication",
                &object.hash,
            )?;
            if actual.bytes() != object.bytes() {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "stored first Backup Publication differs",
                ));
            }
        }
        self.require_first_candidate(prepared, candidate, stop)?;
        // Actual proof files are re-read, with actual bodies, immediately before
        // the metadata barrier. No planned receipt vector can enter the object.
        let observed = payloads.observe(stop)?.into_receipts();
        if canonical_json(&observed)? != canonical_json(&published.payload_receipts)? {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "Backup payload receipts changed before Publication",
            ));
        }
        let mut receipts = Vec::with_capacity(self.sources.len());
        for (id, source) in &self.sources {
            preparation_checkpoint(stop)?;
            source.recheck(stop)?;
            prepared.recheck_names()?;
            let receipt = slots
                .get_mut(id)
                .expect("invariant: every original journal was admitted")
                .publish(&published)?;
            let observed = BackupFirstJournal::inspect(&source.root, &source.disk, candidate)?
                .ok_or_else(|| {
                    missing("Backup Publication journal disappeared after synchronization")
                })?;
            if observed.require_published(&object.hash)? != receipt {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "synchronized Backup Publication changed on readback",
                ));
            }
            receipts.push(receipt);
        }
        drop(slots);
        let seal = PublicationSeal {
            candidate_hash: candidate.hash().into(),
            publication_hash: object.hash.clone(),
            receipts,
        };
        verify_publication_seal(candidate.candidate(), &published, &seal)?;
        let bytes = encode_metadata(&seal, MAX_METADATA)?;
        for source in self.sources.values() {
            preparation_checkpoint(stop)?;
            source.recheck(stop)?;
            install_preparation_bytes(
                &source.root,
                &publication_seal_path(&source.disk, candidate.hash()),
                &bytes,
            )?;
        }
        payloads.local_publications(&published, &seal, true, stop)?;
        self.read_first_publication(prepared, candidate, stop)
    }

    /// Read-only fresh observation. A torn/missing publication or local proof
    /// remains incomplete; this function neither repairs nor issues receipts.
    pub(in crate::engine) fn observe_first_publication(
        &self,
        prepared: &PreparedBackupRegistry,
        candidate: &PreparedBackupFirstCandidate,
        stop: &AtomicBool,
    ) -> Result<BackupFirstPublicationReport> {
        Ok(self
            .read_first_publication(prepared, candidate, stop)?
            .report()
            .clone())
    }

    /// The owned evidence is produced only after fresh actual journal, object,
    /// seal and body observation, never from a prospective receipt vector.
    pub(in crate::engine) fn read_first_publication(
        &self,
        prepared: &PreparedBackupRegistry,
        candidate: &PreparedBackupFirstCandidate,
        stop: &AtomicBool,
    ) -> Result<PreparedBackupFirstPublication> {
        let (finalize, finals) = self.first_finalize_evidence(prepared, candidate, stop)?;
        let payloads = PayloadPlan::new(self, prepared, candidate, &finalize)?;
        let receipts = payloads.observe(stop)?;
        let published = publication(candidate, finalize.clone(), finals, receipts);
        let object = PreparedMetadataObject::new("publication", &published, MAX_METADATA)?;
        let mut receipts = Vec::with_capacity(self.sources.len());
        for source in self.sources.values() {
            source.recheck(stop)?;
            let saved = load_authenticated_object_at(
                &source.root,
                &source.disk,
                "publication",
                &object.hash,
            )?;
            if saved.bytes() != object.bytes() {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "observed Backup Publication differs from its original proofs",
                ));
            }
            let slot = BackupFirstJournal::inspect(&source.root, &source.disk, candidate)?
                .ok_or_else(|| missing("original Backup Publication journal is absent"))?;
            receipts.push(slot.require_published(&object.hash)?);
        }
        let seal = PublicationSeal {
            candidate_hash: candidate.hash().into(),
            publication_hash: object.hash.clone(),
            receipts,
        };
        verify_publication_seal(candidate.candidate(), &published, &seal)?;
        let bytes = encode_metadata(&seal, MAX_METADATA)?;
        for source in self.sources.values() {
            source.recheck(stop)?;
            if source.root.read_bytes(
                &publication_seal_path(&source.disk, candidate.hash()),
                MAX_METADATA,
            )? != bytes
            {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "actual Backup publication seal lacks the exact all-original receipt barrier",
                ));
            }
        }
        payloads.local_publications(&published, &seal, false, stop)?;
        self.require_first_candidate(prepared, candidate, stop)?;
        PreparedBackupFirstPublication::observed(prepared, candidate, published, seal)
    }
}
