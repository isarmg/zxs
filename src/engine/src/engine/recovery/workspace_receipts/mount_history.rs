//! A later ledger frame may qualify an older physical read only with its own
//! original native reserve and FINALIZE. The last Snapshot is a *lookup hint*;
//! it cannot contribute a frame, an actor, a proof of absence, or a decision.
use super::*;
use crate::business::file_draft::worktree::failure;
use crate::persist::snapshot_worktree::{self as codec, Record, mount_change};
use storage_contract::Principal;
use storage_contract::snapshot_worktree::WorktreeId;
use storage_contract::working_tree::MaterialCommitment;

pub(crate) struct NativeMountSuccessor {
    pub(crate) plan: mount_change::Reservation,
    pub(crate) result: mount_change::Publication,
    pub(crate) published: bool,
}

#[derive(Default)]
pub(crate) struct NativeMountHistory {
    rows: BTreeMap<WorktreeId, Vec<NativeMountSuccessor>>,
    last: u64,
}
impl NativeMountHistory {
    pub(crate) fn rows(&self, id: WorktreeId) -> &[NativeMountSuccessor] {
        self.rows.get(&id).map(Vec::as_slice).unwrap_or_default()
    }

    // Preflight must independently reach and validate each selected native
    // transition in forward order before any installed cursor or repair.
    pub(super) fn require_reached(&self, sequence: u64) -> Result<()> {
        if self.last > sequence { return Err(invalid()); }
        Ok(())
    }
}

fn invalid() -> Error { Error::OriginalReceipt(crate::OriginalReceiptError::Candidate) }

fn selected(candidates: &BTreeMap<String, Vec<Receipt>>) -> Result<Option<(&String, &[Receipt])>> {
    let mut decided = candidates.iter().filter(|(_, receipts)| receipts.iter().any(|r|
        matches!(r.record.record_type, RecordType::Accept | RecordType::Publication | RecordType::Finalize)));
    let result = decided.next().map(|(h,rs)| (h,rs.as_slice()));
    if decided.next().is_some() { return Err(invalid()); }
    Ok(result)
}

impl Engine {
    /// Only original native addresses named in the latest candidate need an
    /// extra read. Prior admitted operations are never inferred from its State.
    pub(super) fn read_native_mount_history(
        &self, prepared: &PreparedRecovery,
    ) -> Result<NativeMountHistory> {
        let mut result = NativeMountHistory::default();
        if self.config.generation().is_some() { return Ok(result); }
        let mut found = None;
        for (seq, candidates) in prepared.records.iter().rev() {
            if let Some((h, _)) = selected(candidates)? { found = Some((*seq,h)); break; }
        }
        let Some((end, hash)) = found else { return Ok(result); };
        let last: Candidate = verified_object_at(&self.disk_resources, &prepared.online, "candidate", hash)?;
        self.verify_candidate_membership(&last)?;
        if end != last.sequence { return Err(invalid()); }
        let hints = {
            let snapshot = verified_recovery_snapshot_at(&self.disk_resources, &prepared.online, &last.snapshot_hash)?;
            let state = snapshot.for_candidate(&last)?.state();
            let mut hints: Vec<_> = state.operations.values().filter(|r| matches!(r.kind.as_str(),
                codec::MOUNT_RESERVE | codec::MOUNT_PUBLISH)).map(|r| (r.sequence, r.operation_id)).collect();
            if hints.len() > self.config.max_entries { return Err(invalid()); }
            hints.sort_unstable_by_key(|(seq,_)| *seq);
            hints
        };
        let mut frozen: BTreeMap<OperationId,(mount_change::Reservation, Principal, MaterialCommitment)> = BTreeMap::new();
        for (seq, op) in hints {
            let (hash, receipts) = selected(prepared.records.get(&seq).ok_or_else(invalid)?)?.ok_or_else(invalid)?;
            let candidate: Candidate = verified_object_at(&self.disk_resources, &prepared.online, "candidate", hash)?;
            self.verify_candidate_membership(&candidate)?;
            if candidate.sequence != seq || candidate.operation_id != op || !candidate.versions.is_empty()
                || candidate.generation().is_some() || candidate.registry().is_some() { return Err(invalid()); }
            let snapshot = verified_recovery_snapshot_at(&self.disk_resources, &prepared.online, &candidate.snapshot_hash)?;
            let next = snapshot.for_candidate(&candidate)?.state();
            verify_current_candidate_receipt(&candidate, next)?;
            let native = next.operations.get(&op).ok_or_else(invalid)?;
            let actor: Principal = next.operation_principals.get(&op).ok_or_else(invalid)?.parse().map_err(|_| invalid())?;
            let decoded = codec::decode(native).map_err(|e| failure(crate::snapshot_worktree::RegistrationError::Codec(e)))?
                .ok_or_else(invalid)?;
            if !decoded.protection.is_empty() || decoded.membership_epoch != self.config.membership_epoch()
                || decoded.receipt(op, storage_contract::Sequence::new(seq), &actor)
                    .map_err(|e| failure(crate::snapshot_worktree::RegistrationError::Codec(e)))? != *native
            { return Err(invalid()); }
            let publication: Option<Publication> = receipts.iter().find(|r| r.record.record_type == RecordType::Publication)
                .map(|r| verified_object_at(&self.disk_resources, &prepared.online, "publication", &r.record.object_hash)).transpose()?;
            if let Some(p) = &publication {
                if p.result != *native || p.recovery.is_some() || p.candidate_hash != *hash { return Err(invalid()); }
                crate::journal::verify_finalized_publication(&candidate, p)?;
            }
            match decoded.record {
                Record::ReserveCleanMount(plan) => {
                    let Some(p) = publication else { continue; };
                    published_barrier(self, &prepared.online, receipts, &candidate, &p)?;
                    let commitment = Record::ReserveCleanMount(plan.clone()).commitment()
                        .map_err(|e| failure(crate::snapshot_worktree::RegistrationError::Codec(e)))?;
                    if plan.request.operation_id != op || frozen.insert(op,(plan,actor,commitment)).is_some() {
                        return Err(invalid());
                    }
                }
                Record::PublishCleanMount(value) => {
                    let (plan, owner, commitment) = frozen.remove(&value.reserve).ok_or_else(invalid)?;
                    if owner != actor || commitment != value.reservation || plan.publish_operation != op
                        || crate::directory_worktree::ledger::mount_checkpoint(&plan)
                            .map_err(|e| failure(crate::snapshot_worktree::RegistrationError::Ledger(e)))? != value.checkpoint
                    { return Err(invalid()); }
                    // Only actual FINALIZE from every fixed member can attest
                    // the physical installation, never a visible State or
                    // self-contained publication object without its journals.
                    if !self.content_fully_finalized(&candidate, hash, next, &prepared.online, receipts)? {
                        if publication.is_some() { return Err(invalid()); }
                        continue;
                    }
                    let rows = result.rows.entry(plan.request.target.worktree_id).or_default();
                    if let Some(prior) = rows.last()
                        && (!prior.published || prior.result.checkpoint != plan.before
                            || prior.plan.content_reserve != plan.content_reserve || prior.plan.host != plan.host
                            || prior.plan.instance != plan.instance || prior.plan.ledger != plan.ledger)
                    { return Err(invalid()); }
                    rows.push(NativeMountSuccessor { plan, result: value, published: publication.is_some() });
                    result.last = seq;
                }
                _ => return Err(invalid()),
            }
        }
        Ok(result)
    }
}
