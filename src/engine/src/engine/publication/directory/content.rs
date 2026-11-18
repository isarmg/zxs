//! Budget one authorized original-content preparation before EXCL. This
//! budget is neither a commit decision nor a reusable filesystem capability.
use super::super::*;
use super::Failure;
use crate::business::file_draft::worktree::failure;
use crate::snapshot_worktree::{OriginalContentInput, PreparedOriginalContent};
use crate::working_tree::snapshot::{SnapshotNodeBinding, SnapshotReference};
use storage_contract::working_tree::DirectoryId;

pub(crate) struct BudgetedOriginalContent {
    preparation: DirectoryId,
    source: SnapshotNodeBinding,
    target: SnapshotReference,
    operation: OperationId,
    _descriptors: PayloadProofReservation,
}
impl BudgetedOriginalContent {
    pub(crate) fn require(&self, input: &OriginalContentInput) -> Result<()> {
        if self.preparation != input.preparation() || self.source != input.plan().source_binding
            || self.target != input.manifest().reference() || self.operation != input.plan().publish_operation
        { return Err(Failure::ContainerScope.into()); }
        Ok(())
    }
}

impl crate::engine::PlanningContext<'_> {
    pub(crate) fn prepare_sparse_content_node(&self, input: OriginalContentInput, maximum: usize)
        -> Result<(PreparedOriginalContent, State, Vec<Prepared>)>
    {
        input.require_context(self).map_err(failure)?;
        let owner = self.config.disks.iter().find(|d| d.disk_id == input.plan().host.registration.disk_id)
            .ok_or(Failure::Registration)?;
        let mut required = PhysicalRequirements::new(self);
        // FILE may already hold a verified independent full copy left by an
        // interrupted preparation of this frozen version. The native FILE
        // preparer separately reopens and verifies it; never adopt an unknown
        // temporary directory or use a same-hash file in place of the edit.
        drop(self.observe_existing_payloads(input.prepared(), &mut required)?);
        required.add(owner, input.directory_bytes().map_err(failure)?)?;
        let envelope = input.result_budget().map_err(failure)?;
        if envelope > maximum { return Err(Failure::ContainerBudget.into()); }
        let mut additional = envelope as u64;
        let snapshot = prepare_snapshot(input.state(), maximum)?;
        snapshot.visit_prepared_objects(&mut |_, _, bytes| {
            additional = additional.checked_add(bytes.len() as u64).ok_or(Failure::PhysicalBytesOverflow)?;
            Ok(())
        })?;
        let (_, histories) = crate::history_package::prepare_history_manifests(input.state(), maximum)?;
        for history in histories {
            additional = additional.checked_add(history.bytes().len() as u64).ok_or(Failure::PhysicalBytesOverflow)?;
        }
        for member in self.members() { required.add(&member, additional)?; }
        required.check()?;
        let permit = BudgetedOriginalContent { preparation: input.preparation(), source: input.plan().source_binding,
            target: input.manifest().reference(), operation: input.plan().publish_operation,
            _descriptors: payload_proof_budget().reserve(3).ok_or(Failure::ContainerBudget)? };
        input.stage(self, permit).map_err(failure)
    }
}

/// Transient capability from this native slot's durable FINALIZE on
/// every fixed member. Neither a reserve,
/// local ledger, prepared snapshot, arbitrary Finalize value nor candidate
/// alone can create this grant.
pub(crate) struct FinalizedOriginalContent<'a> {
    candidate: &'a Candidate,
    receipt: &'a OperationResult,
}
impl<'a> FinalizedOriginalContent<'a> {
    pub(in crate::engine) fn from_members(
        candidate: &'a Candidate, hash: &str, next: &'a State, finalize: &Finalize,
        receipts: &[Receipt], members: &[DiskConfig],
    ) -> Result<Self> {
        let result = next.operations.get(&candidate.operation_id).ok_or(Failure::ContainerScope)?;
        let expected_hash = object_hash("candidate", candidate)?;
        let finalized = object_hash("finalize", finalize)?;
        if result.kind != crate::persist::snapshot_worktree::CONTENT_PUBLISH
            || result.sequence != candidate.sequence || result.request_digest != candidate.request_digest
            || hash != expected_hash || finalize.candidate_hash != hash || finalize.recovery_id.is_some()
            || members.is_empty() || members.len() != candidate.metadata_disk_ids.len()
            || finalize.accepts.len() != members.len() || receipts.len() != members.len()
        { return Err(Failure::ContainerScope.into()); }
        let mut distinct = std::collections::BTreeSet::new();
        for ((disk, accepted), finalized_receipt) in members.iter().zip(&finalize.accepts).zip(receipts) {
            if !distinct.insert(&disk.disk_id) || !candidate.metadata_disk_ids.contains(&disk.disk_id)
                || accepted.disk_id != disk.disk_id || finalized_receipt.disk_id != disk.disk_id
                || accepted.slot_id != disk.slot_id || finalized_receipt.slot_id != disk.slot_id
                || !crate::journal::receipt_binds(accepted, RecordType::Accept, candidate.sequence, hash, hash)?
                || !crate::journal::receipt_binds(finalized_receipt, RecordType::Finalize, candidate.sequence, hash, &finalized)?
            { return Err(Failure::ContainerScope.into()); }
        }
        Ok(Self { candidate, receipt: result })
    }

    pub(crate) fn require(&self, plan: &crate::persist::snapshot_worktree::content_batch::Reservation,
        value: &crate::persist::snapshot_worktree::content_batch::Publication) -> Result<()> {
        use crate::persist::snapshot_worktree::{self as codec, Record};
        let decoded = codec::decode(self.receipt).map_err(|e| failure(crate::snapshot_worktree::RegistrationError::Codec(e)))?
            .ok_or(Failure::ContainerScope)?;
        if decoded.record != Record::PublishContentBatch(value.clone())
            || plan.publish_operation != self.candidate.operation_id || plan.request.operation_id != value.reserve
            || value.reservation != Record::ReserveContentBatch(plan.clone()).commitment()
                .map_err(|e| failure(crate::snapshot_worktree::RegistrationError::Codec(e)))?
            || self.candidate.versions.len() != value.rows.len()
            || self.candidate.versions.iter().zip(&value.rows).any(|(a,b)|
                a.version != b.content.version || a.placement != b.content.placement)
        { return Err(Failure::ContainerScope.into()); }
        Ok(())
    }
}

impl Engine {
    /// Under the one engine writer, all original fixed members need a ready,
    /// still-named journal and no later slot than the admitted State. A crash
    /// after PREPARE cannot start another provisional node for this request.
    pub(crate) fn require_unstarted_content_slot(&self, reserve: OperationId) -> Result<()> {
        self.require_no_commit_preparation()?;
        let ctx = self.planning_context();
        if ctx.blocked || ctx.current_namespaces.is_some() || ctx.config.generation().is_some()
            || ctx.next_slot <= ctx.state.sequence
        { return Err(failure(crate::snapshot_worktree::RegistrationError::Pending(reserve))); }
        let members = ctx.members();
        if members.is_empty() { return Err(Failure::Registration.into()); }
        for member in members {
            ctx.disk_resources.get(&member)?.root.held_identity()
                .map_err(|_| Failure::ContainerIdentity)?;
            let journal = self.journals.get(&member.disk_id).ok_or(Failure::Registration)?;
            journal.verify_retained_name()?;
            if journal.needs_recovery() || journal.records().last().is_some_and(|(r,_)| r.sequence > ctx.state.sequence) {
                return Err(failure(crate::snapshot_worktree::RegistrationError::Pending(reserve)));
            }
        }
        Ok(())
    }

    /// One actual business Candidate. Neither the reserve nor any individual
    /// FILE row can be used as a standalone mutation or directory decision.
    pub(crate) fn publish_sparse_content(&mut self, input: OriginalContentInput) -> Result<OperationResult> {
        use crate::persist::snapshot_worktree::{PublicationEncoding, Record};
        self.require_unstarted_content_slot(input.plan().request.operation_id)?;
        let (held, projected, versions) = self.planning_context().prepare_sparse_content_node(input, MAX_METADATA)?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("after_worktree_content_stage")?;
        let record = Record::PublishContentBatch(held.result().clone());
        let encoded = PublicationEncoding::new(&record).map_err(|e| failure(crate::snapshot_worktree::RegistrationError::Codec(e)))?;
        let op = held.plan().publish_operation;
        let plan = self.plan_mutation_digest(record.kind(), held.principal().as_str(), op, encoded.request(), None,
            |_, next, prepared| {
                if next.sequence != projected.sequence || next.business_generation != projected.business_generation
                    || !prepared.is_empty() { return Err(Failure::ContainerScope.into()); }
                *next = projected; *prepared = versions; Ok(encoded.result())
            })?;
        let MutationPlan::Prepared { next, prepared, result } = plan else { return Err(Failure::ContainerScope.into()); };
        let publication = self.planning_context().prepare_publication_domains(*next, prepared, result,
            MAX_METADATA, MAX_METADATA, None, Some(Box::new(held)))?;
        self.decide_publication(publication)
    }
}
