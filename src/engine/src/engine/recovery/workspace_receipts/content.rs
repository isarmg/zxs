//! Read actual original sparse-content outcomes against the preceding native
//! receipt cursor, never against checkpoint State or an isolated local ledger.
use super::*;
use crate::business::file_draft::worktree::failure;
use crate::snapshot_worktree::ObservedOriginalContent;

impl Engine {
    /// Most solitary PREPAREs keep their ordinary abandonable semantics.
    /// Consult only a published sparse reservation from this exact prefix;
    /// names on the disk, later States and user-supplied UUIDs cannot select it.
    pub(super) fn original_content_prepare(&self, snapshot: &Option<String>, online: &[DiskConfig],
        sequence: u64, candidates: &BTreeMap<String, Vec<Receipt>>,
        fences: &crate::snapshot_worktree::ContentReplayFences) -> Result<Option<String>> {
        if fences.is_empty() { return Ok(None); }
        let mut selected = None;
        for (hash, receipts) in candidates {
            if receipts.is_empty() || !receipts.iter().all(|r| r.record.record_type == RecordType::Prepare) { continue; }
            let candidate: Candidate = verified_object_at(&self.disk_resources, online, "candidate", hash)?;
            if !fences.selects(candidate.operation_id) { continue; }
            self.verify_candidate_membership(&candidate)?;
            if selected.is_some() || candidate.sequence != sequence || &candidate.previous_snapshot != snapshot
            { return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate)); }
            for r in receipts {
                if !crate::journal::receipt_binds(r, RecordType::Prepare, sequence, hash, &candidate.snapshot_hash)? {
                    return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate));
                }
            }
            selected = Some(hash.clone());
        }
        Ok(selected)
    }

    /// Reserve freezes identities but cannot publish a FILE, directory, new
    /// working state, permissions or any other business change. Reconstruct
    /// that complete transition from the authenticated predecessor.
    pub(super) fn require_content_reservation(&self, candidate: &Candidate, prefix: &ReceiptPrefix<'_>,
        admitted: &crate::publication_evidence::AdmittedSnapshot) -> Result<()> {
        use crate::persist::snapshot_worktree::{self as codec, Record};
        let next = admitted.for_candidate(candidate)?.state();
        let result = next.operations.get(&candidate.operation_id).ok_or(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate))?;
        let actor = next.operation_principals.get(&candidate.operation_id).ok_or(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate))?
            .parse::<storage_contract::Principal>().map_err(|_| Error::OriginalReceipt(crate::OriginalReceiptError::Candidate))?;
        let decoded = codec::decode(result).map_err(|e| failure(crate::snapshot_worktree::RegistrationError::Codec(e)))?
            .ok_or(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate))?;
        let Record::ReserveContentBatch(plan) = &decoded.record else { return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate)); };
        if result.operation_id != plan.request.operation_id || prefix.namespaces.is_some() || self.config.generation().is_some()
            || candidate.generation().is_some() || !candidate.versions.is_empty() || result.sequence != candidate.sequence
            || candidate.request_digest != result.request_digest || decoded.membership_epoch != self.config.membership_epoch()
            || !decoded.protection.is_empty()
            || decoded.receipt(result.operation_id, storage_contract::Sequence::new(result.sequence), &actor)
                .map_err(|e| failure(crate::snapshot_worktree::RegistrationError::Codec(e)))? != *result
        { return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate)); }
        let source = crate::snapshot_worktree::ReceiptIndex::capture_with_originals(prefix.state, prefix.originals).map_err(failure)?;
        source.require_planned(next, result, &actor, &decoded.record).map_err(failure)?;
        crate::business::file_draft::worktree_content::require_frozen(prefix.state, plan)?;
        let mut projected = crate::business::file_draft::unborn::begin(prefix.state, storage_contract::Sequence::new(result.sequence))?;
        crate::container_storage::capture_changed_names(prefix.state, &mut projected);
        let projected = crate::business::file_draft::unborn::finish_state(prefix.state, projected, result, &actor)?;
        if !crate::publication_shape::same_transition(prefix.state, &projected, next) {
            return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate));
        }
        Ok(())
    }

    /// Observation only. A mixed MOVE position is possible only after the
    /// actual fixed-member FINALIZE on the exact original Candidate. The
    /// installer obtains a fresh native proof after reproducing normal stages.
    pub(in crate::engine::recovery) fn content_fully_finalized(&self, candidate: &Candidate,
        hash: &str, next: &State, online: &[DiskConfig], receipts: &[Receipt]) -> Result<bool> {
        let members = self.members();
        let mut finals = Vec::with_capacity(members.len());
        for disk in &members {
            let mut found = receipts.iter().filter(|r| r.disk_id == disk.disk_id
                && r.record.record_type == RecordType::Finalize);
            let Some(receipt) = found.next() else { return Ok(false); };
            if found.next().is_some() {
                return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate));
            }
            finals.push(receipt.clone());
        }
        let Some(first) = finals.first() else {
            return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate));
        };
        let finalize: crate::journal::Finalize = verified_object_at(&self.disk_resources, online,
            "finalize", &first.record.object_hash)?;
        if !finalize.accepts.iter().all(|r| receipts.iter().any(|seen| seen == r)) {
            return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate));
        }
        crate::engine::FinalizedOriginalContent::from_members(candidate, hash, next, &finalize, &finals, &members)?;
        Ok(true)
    }

    pub(super) fn read_content_receipt_candidate(&self, candidate: Candidate,
        admitted: crate::publication_evidence::AdmittedSnapshot, publication: Option<Publication>,
        prefix: ReceiptPrefix<'_>, online: &[DiskConfig], hash: &str, receipts: &[Receipt],
    ) -> Result<PreparedCandidate> {
        let source = DirectoryReplayPrefix::capture(self, &prefix)?;
        let finalized = if publication.is_none() {
            self.content_fully_finalized(&candidate, hash, admitted.for_candidate(&candidate)?.state(), online, receipts)?
        } else { false };
        ObservedOriginalContent::preflight(&source, &candidate, &admitted, finalized, publication.is_some())
            .map_err(failure)?;
        if let Some(p) = &publication {
            if p.recovery.is_some() || p.result != *admitted.for_candidate(&candidate)?.state().operations
                .get(&candidate.operation_id).ok_or(Error::OriginalReceipt(crate::OriginalReceiptError::Candidate))?
            { return Err(Error::OriginalReceipt(crate::OriginalReceiptError::Visible)); }
        }
        // A database or apparent node alone does not supply this observation.
        // The normal caller authenticates the complete Publication and seal
        // before the marker can ever enter a covered checkpoint.
        let directory_admission = publication.as_ref().map(|p|
            directory::DirectoryCandidateAdmission::capture(self, &candidate, p)).transpose()?;
        Ok(PreparedCandidate { candidate, publication, barrier_failure: None, snapshot: Some(admitted.into_shared()),
            next_namespaces: None, directory_admission })
    }
}
