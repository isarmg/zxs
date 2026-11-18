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
        if !matches!(result.kind.as_str(), crate::persist::snapshot_worktree::CONTENT_PUBLISH
            | crate::persist::snapshot_worktree::MOUNT_PUBLISH)
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

    /// The same fixed member decision, restricted to this original mount plan.
    /// A FILE batch proof, foreign reserve or generic metadata result cannot append.
    pub(crate) fn require_mount(
        &self, plan: &crate::persist::snapshot_worktree::mount_change::Reservation,
        value: crate::persist::snapshot_worktree::mount_change::Publication,
    ) -> Result<()> {
        use crate::persist::snapshot_worktree::{self as codec, Record};
        let decode = |source| failure(crate::snapshot_worktree::RegistrationError::Codec(source));
        let decoded = codec::decode(self.receipt).map_err(decode)?.ok_or(Failure::ContainerScope)?;
        if decoded.record != Record::PublishCleanMount(value) || !decoded.protection.is_empty()
            || !self.candidate.versions.is_empty() || self.candidate.operation_id != plan.publish_operation
            || value.reserve != plan.request.operation_id
            || value.reservation != Record::ReserveCleanMount(plan.clone()).commitment().map_err(decode)?
        {
            return Err(Failure::ContainerScope.into());
        }
        Ok(())
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
    /// A new request has no permanent Pending operation yet. A still-bound
    /// original does. A registered other candidate or an unknown journal can
    /// never be treated as proof that this proposed publication is unstarted.
    pub(crate) fn require_unstarted_content_slot(&self, request: &storage_contract::snapshot_worktree::CommitWorktree,
        principal: &storage_contract::Principal, publish: Option<OperationId>) -> Result<()> {
        self.require_unstarted_directory_slot(request.operation_id, principal, publish, |record| {
            matches!(record, crate::persist::snapshot_worktree::Record::ReserveContentBatch(plan)
                if plan.request == *request)
        })
    }

    pub(crate) fn require_unstarted_mount_slot(
        &self, request: &storage_contract::snapshot_worktree::mount_change::MountChange,
        principal: &storage_contract::Principal, publish: Option<OperationId>,
    ) -> Result<()> {
        self.require_unstarted_directory_slot(request.operation_id, principal, publish, |record| {
            matches!(record, crate::persist::snapshot_worktree::Record::ReserveCleanMount(plan)
                if plan.request == *request)
        })
    }

    fn require_unstarted_directory_slot(
        &self, operation: OperationId, principal: &storage_contract::Principal,
        publish: Option<OperationId>, same_request: impl Fn(&crate::persist::snapshot_worktree::Record) -> bool,
    ) -> Result<()> {
        let held = || match publish {
            Some(_) => failure(crate::snapshot_worktree::RegistrationError::Pending(operation)),
            None => Error::new(ErrorCode::RecoveryRequired, "native decision history must be verified before reserving this worktree"),
        };
        self.require_no_commit_preparation()?;
        let ctx = self.planning_context();
        if ctx.blocked || ctx.current_namespaces.is_some() || ctx.config.generation().is_some()
            || ctx.next_slot <= ctx.state.sequence { return Err(held()); }
        let members = ctx.members();
        if members.is_empty() { return Err(Failure::Registration.into()); }
        let mut future = false;
        for member in &members {
            ctx.disk_resources.get(member)?.root.held_identity().map_err(|_| Failure::ContainerIdentity)?;
            let journal = self.journals.get(&member.disk_id).ok_or(Failure::Registration)?;
            journal.verify_retained_name()?;
            if journal.needs_recovery() { return Err(held()); }
            future |= journal.records().last().is_some_and(|(r,_)| r.sequence > ctx.state.sequence);
        }
        if !future { return Ok(()); }
        // Rare abandoned ordinary PREPARE. The cached journal tail alone is
        // not enough: read every fixed member's original verified suffix, so
        // another PREPARE cannot hide this reserved publish hash at an older
        // slot. Protocol-native records already have per-journal read bounds.
        let index = ctx.snapshot_receipts().map_err(failure)?;
        let frozen = index.content_replay_fences();
        let mut seen = std::collections::BTreeSet::new();
        for member in &members {
            let root = &ctx.disk_resources.get(member)?.root;
            ctx.config.verify_root_membership(root, member)?;
            let scan = crate::journal::OpenJournal::scan_configured_at(root, member, ctx.config)?.scan;
            if scan.torn_tail { return Err(held()); }
            for (record, frame_hash) in scan.records.into_iter().filter(|(r, _)| r.sequence > ctx.state.sequence) {
                if record.record_type != RecordType::Prepare { return Err(held()); }
                let candidate: Candidate = crate::publication_evidence::verified_object_at(ctx.disk_resources, &members,
                    "candidate", &record.candidate_hash)?;
                let receipt = Receipt { disk_id: member.disk_id.clone(), slot_id: member.slot_id.clone(),
                    frame_hash, record };
                if !crate::journal::receipt_binds(&receipt, RecordType::Prepare, candidate.sequence,
                    &receipt.record.candidate_hash, &candidate.snapshot_hash)? || candidate.previous_snapshot != *ctx.snapshot
                { return Err(held()); }
                self.verify_candidate_membership(&candidate)?;
                if frozen.selects(candidate.operation_id) || publish == Some(candidate.operation_id) { return Err(held()); }
                if !seen.insert(receipt.record.candidate_hash) { continue; }
                if seen.len() > ctx.config.max_entries { return Err(held()); }
                if candidate.operation_id == operation {
                    // The prior occupancy attempt has no native ACCEPT on any
                    // fixed member. Do not silently rebind another principal,
                    // purpose or request, or adopt its uncommitted allocations.
                    let snapshot = crate::publication_evidence::verified_recovery_snapshot_at(ctx.disk_resources, &members,
                        &candidate.snapshot_hash)?;
                    let next = snapshot.for_candidate(&candidate)?.state();
                    let result = next.operations.get(&candidate.operation_id).ok_or_else(&held)?;
                    let decoded = crate::persist::snapshot_worktree::decode(result)
                        .map_err(|e| failure(crate::snapshot_worktree::RegistrationError::Codec(e)))?
                        .ok_or_else(&held)?;
                    if publish.is_some() || candidate.generation().is_some() || !candidate.versions.is_empty()
                        || next.operation_principals.get(&candidate.operation_id).map(String::as_str) != Some(principal.as_str())
                        || !same_request(&decoded.record)
                        || decoded.record.digest(principal).map_err(|e| failure(crate::snapshot_worktree::RegistrationError::Codec(e)))?
                            != candidate.request_digest
                    { return Err(failure(crate::snapshot_worktree::RegistrationError::Digest(operation))); }
                }
            }
        }
        // Recheck the original live descriptor identity after the independent
        // scan. Success creates no receipt, and never authorizes a MOVE.
        for member in &members {
            self.journals.get(&member.disk_id).ok_or(Failure::Registration)?.verify_retained_name()?;
        }
        Ok(())
    }

    /// One actual business Candidate. Neither the reserve nor any individual
    /// FILE row can be used as a standalone mutation or directory decision.
    pub(crate) fn publish_sparse_content(&mut self, input: OriginalContentInput) -> Result<OperationResult> {
        use crate::persist::snapshot_worktree::{PublicationEncoding, Record};
        self.require_unstarted_content_slot(&input.plan().request, input.principal(), Some(input.plan().publish_operation))?;
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
            MAX_METADATA, MAX_METADATA, None, Some(Box::new(held)), None)?;
        let original_slot = self.next_slot;
        let outcome = self.decide_publication(publication);
        // A PREPARE may have reached even one member before a later ordinary
        // I/O error. Keep all further writers out until a cold scan sees this
        // exact slot or proves that no business Candidate ever existed.
        if outcome.is_err() && self.next_slot != original_slot { self.blocked = true; }
        outcome
    }
}

impl Engine {
    pub(crate) fn publish_clean_mount(
        &mut self, prepared: crate::snapshot_worktree::mount::PreparedMount,
    ) -> Result<OperationResult> {
        use crate::persist::snapshot_worktree::{PublicationEncoding, Record};
        let held = &prepared.held;
        let op = held.plan().publish_operation;
        self.require_unstarted_mount_slot(&held.plan().request, held.principal(), Some(op))?;
        let record = Record::PublishCleanMount(held.result());
        let encoded = PublicationEncoding::new(&record)
            .map_err(|e| failure(crate::snapshot_worktree::RegistrationError::Codec(e)))?;
        let mutation = self.plan_mutation_digest(
            record.kind(), held.principal().as_str(), op, encoded.request(), None,
            |_, _, _| Ok(encoded.result()),
        )?;
        let MutationPlan::Prepared { next, prepared: files, result } = mutation
            else { return Err(Failure::ContainerScope.into()); };
        let publication = self.planning_context().prepare_publication_domains(
            *next, files, result, MAX_METADATA, MAX_METADATA, None, None, Some(Box::new(prepared)),
        )?;
        let slot = self.next_slot;
        let outcome = self.decide_publication(publication);
        if outcome.is_err() && self.next_slot != slot { self.blocked = true; }
        outcome
    }
}
