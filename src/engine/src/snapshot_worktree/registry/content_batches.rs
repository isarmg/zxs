//! Permanent occupancy and one complete FILE+directory result. An index entry
//! identifies the original request; it cannot authorize any local move.
use super::{
    Allocations, CompletionOwner, NodePlan, Publication, PublishedNode, Receipt, ReceiptIndex,
    edits::LedgerHead, require_completion,
};
use crate::business::file_draft::Allocation;
use crate::directory_worktree::ledger::{LedgerError, content_checkpoints};
use crate::directory_worktree::projection::FileResult;
use crate::persist::snapshot_worktree::{NodeAllocation, content_batch::{Publication as ResultRecord, Reservation}};
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::working_tree::snapshot::{AdmittedSnapshotManifest, SnapshotEntryContent, SnapshotFileSource, MAX_VERSIONS};
use std::collections::BTreeSet;
use storage_contract::{FileNodeId, OperationId, Principal};
use storage_contract::file_draft::FileWorkingState;
use storage_contract::snapshot_worktree::{CommitWorktree, SnapshotHandle};
use storage_contract::working_tree::MaterialCommitment;

pub(super) struct Complete {
    pub(super) value: ResultRecord,
    pub(super) node: NodePlan,
    manifest: AdmittedSnapshotManifest,
}
pub(super) struct BatchRecord<'a> {
    pub(super) receipt: Receipt<'a>,
    pub(super) plan: Reservation,
    pub(super) commitment: MaterialCommitment,
    completion: Publication<Complete>,
}
impl BatchRecord<'_> {
    pub(super) fn complete(&self) -> Option<&Complete> {
        match &self.completion { Publication::Reserved => None, Publication::Complete(value) => Some(value) }
    }
    pub(super) fn require_publication(&self, published: &crate::directory_worktree::lifecycle::Publication,
        files: &[FileResult]) -> std::result::Result<(), LedgerError>
    {
        let value = self.complete().ok_or(LedgerError::PublicationPending)?;
        if *published != self.plan.ledger_publication()
            || !value.value.rows.iter().map(|row| &row.file).eq(files.iter()) {
            return Err(LedgerError::FileResults);
        }
        Ok(())
    }
}

/// Constructible only by original/index lookup with the same principal/intent.
pub(in crate::snapshot_worktree) struct FrozenContent<'a> {
    plan: &'a Reservation,
    principal: &'a Principal,
    commitment: MaterialCommitment,
}
impl FrozenContent<'_> {
    pub(in crate::snapshot_worktree) fn belongs_to(&self, principal: &Principal) -> bool { self.principal == principal }
    pub(in crate::snapshot_worktree) fn plan(&self) -> &Reservation { self.plan }
    pub(in crate::snapshot_worktree) fn commitment(&self) -> MaterialCommitment { self.commitment }
}
pub(in crate::snapshot_worktree) enum ContentRetry<'a> {
    Reserved(FrozenContent<'a>),
    Complete(SnapshotHandle),
}
impl<'a> ReceiptIndex<'a> {
    pub(in crate::snapshot_worktree) fn retry_content(&self, request: &CommitWorktree, principal: &Principal)
        -> Result<Option<ContentRetry<'_>>>
    {
        let Some(entry) = self.content_batches.get(&request.operation_id) else { return Ok(None); };
        if &entry.receipt.principal != principal || entry.plan.request != *request {
            return Err(RegistrationError::Digest(request.operation_id));
        }
        Ok(Some(match &entry.completion {
            Publication::Reserved => ContentRetry::Reserved(FrozenContent {
                plan: &entry.plan, commitment: entry.commitment, principal: &entry.receipt.principal,
            }),
            Publication::Complete(_) => ContentRetry::Complete(entry.plan.handle()),
        }))
    }

    pub(super) fn reserve_content(&mut self, receipt: Receipt<'a>, plan: Reservation,
        commitment: MaterialCommitment, allocations: &mut Allocations) -> Result<()>
    {
        let op = receipt.original.operation_id;
        let target = plan.request.target;
        let registered = self.worktree(target)?;
        let source = registered.source();
        let edits = self.active_edits(target);
        if self.completions.contains_key(&op) || self.planned_files.contains(&op)
            || op != plan.request.operation_id || op == plan.publish_operation
            || self.content_instances.contains_key(&target.worktree_id)
            || !plan.host.directories.is_empty() || plan.host != *source.host() || plan.root != source.initialization().host
            || plan.source != source.handle() || plan.source_binding != source.binding()
            || plan.source != self.commit_head(plan.source)? || plan.destination_node == plan.source.node_id
            || registered.published_node().is_some() || !registered.added_files().is_empty()
            || registered.revision() != plan.request.expected || registered.binding() != plan.ledger
            || registered.checkpoint() != plan.before || registered.reservation().request.parent_directory_id != plan.root.directory_id
            || plan.instance.operation() != registered.reservation().request.operation_id
            || plan.instance.worktree() != target.worktree_id || plan.instance.source() != plan.ledger.source()
            || edits.is_empty() || edits.len() > MAX_VERSIONS || edits.len() != plan.rows.len()
        { return Err(RegistrationError::CommitScope); }
        plan.before.require_publication_frames().map_err(RegistrationError::Ledger)?;
        let mut files = BTreeSet::new(); let mut entries = BTreeSet::new();
        let mut native = BTreeSet::new(); let mut ids = BTreeSet::new();
        let mut branches = BTreeSet::from([plan.host.namespace.branch_id]);
        let mut last = None;
        for (input, edit) in plan.rows.iter().zip(edits) {
            let (reserved, done) = self.original_edit(target, edit)?;
            if last.is_some_and(|last| last >= edit.path.key()) || input.edit != *edit
                || input.edit_operation != done.reserve || input.edit_completion != reserved.completion
                || input.original != reserved.original || input.original.file_id != edit.material.file_id
                || input.base.version_id() != input.original.version_id
                || reserved.original_source != (SnapshotFileSource::Container {
                    branch: input.base.branch_id(), node: input.base.node_id(),
                })
                || input.namespace.repository != plan.host.namespace.repository_id
                || input.namespace.branch != plan.host.namespace.branch_id
                || input.namespace.name.as_str() != edit.path.file_name() || input.namespace.parent.ordinary().is_none()
                || input.owner.to_string() != plan.host.registration.disk_id || input.owner_root != plan.root.root_id
                || input.owner_entity.inode() == 0 || !native.insert(input.owner_entity)
                || !entries.insert(input.namespace.entry) || !files.insert(input.original.file_id)
                || !branches.insert(input.base.branch_id())
                || !matches!(input.clean, FileWorkingState::Automatic { .. } | FileWorkingState::ManualClean { .. })
                || !input.allocations.contains(&Allocation::Commit(input.commit))
                || !input.allocations.contains(&Allocation::Tree(input.tree))
                || input.allocations.iter().any(|a| !ids.insert(*a))
            { return Err(RegistrationError::CommitScope); }
            last = Some(edit.path.key());
        }
        source.manifest().validate_material_replacements(&edits.iter().map(|e|
            (e.path.clone(), e.material)).collect::<Vec<_>>()).map_err(RegistrationError::Manifest)?;
        if branches.iter().any(|id| self.frozen_content.contains_key(id))
            || self.content_batches.values().any(|e| e.complete().is_none()
                && e.plan.rows.iter().flat_map(|r| &r.allocations).any(|id| ids.contains(id)))
            || !allocations.nodes.insert(plan.destination_node)
        { return Err(RegistrationError::CommitScope); }
        self.reserve_completion(op, plan.publish_operation, CompletionOwner::ContentBatch(op))?;
        for id in branches { self.frozen_content.insert(id, op); }
        self.pending_edits.insert(target.worktree_id, op);
        self.pending_branches.insert((plan.source.repository_id, plan.source.branch_id), op);
        self.content_instances.insert(target.worktree_id, op);
        self.content_batches.insert(op, BatchRecord { receipt, plan, commitment, completion: Publication::Reserved });
        Ok(())
    }

    pub(in crate::snapshot_worktree) fn require_pending_content(&self, state: &crate::State) -> Result<()> {
        for entry in self.content_batches.values().filter(|entry| entry.complete().is_none()) {
            if state.operations.contains_key(&entry.plan.publish_operation) {
                return Err(RegistrationError::Pending(entry.plan.request.operation_id));
            }
            crate::business::file_draft::worktree_content::require_frozen(state, &entry.plan)
                .map_err(RegistrationError::Publication)?;
        }
        Ok(())
    }

    pub(super) fn content_node(&self, reserve: OperationId, handle: SnapshotHandle) -> Result<PublishedNode<'_>> {
        let entry = self.content_batches.get(&reserve).ok_or(RegistrationError::Source)?;
        if entry.plan.handle() != handle { return Err(RegistrationError::Source); }
        let complete = entry.complete().ok_or(RegistrationError::Pending(reserve))?;
        Ok(PublishedNode { plan: &complete.node, manifest: &complete.manifest, binding: complete.value.prepared_node.binding })
    }
}

impl<'a> ReceiptIndex<'a> {
    pub(super) fn publish_content(&mut self, receipt: Receipt<'a>, value: ResultRecord,
        protection: &[crate::persist::file_draft::Protection]) -> Result<()>
    {
        let entry = self.content_batches.get(&value.reserve).ok_or(RegistrationError::Source)?;
        let plan = &entry.plan;
        let operation = receipt.original.operation_id;
        require_completion(&entry.receipt, &receipt, plan.publish_operation, entry.commitment, value.reservation)?;
        let source = self.node(plan.source)?;
        let key = (plan.source.repository_id, plan.source.branch_id);
        if entry.complete().is_some() || self.pending_branches.get(&key) != Some(&value.reserve)
            || self.pending_edits.get(&plan.request.target.worktree_id) != Some(&value.reserve)
            || self.branch_heads.get(&key) != Some(&plan.source)
            || !matches!(self.completions.get(&operation), Some(CompletionOwner::ContentBatch(owner)) if *owner == value.reserve)
            || value.rows.len() != plan.rows.len() || value.rows.len() != protection.len()
            || !value.prepared_node.binding.independent_node(plan.source_binding)
            || value.prepared_node.preparation == plan.root.directory_id
            || self.directory_ids.contains_key(&value.prepared_node.preparation)
        { return Err(RegistrationError::Completion(operation)); }
        let mut entries = source.manifest().entries().to_vec();
        let mut expected_protection = Vec::with_capacity(plan.rows.len());
        for (input, output) in plan.rows.iter().zip(&value.rows) {
            let material = input.edit.material;
            let version = &output.content.version;
            let committed = self.source.commits.get(&input.commit.to_string()).ok_or(RegistrationError::CommitScope)?;
            if output.path != input.edit.path || output.file.path != output.path
                || output.file.material != material || input.commit != output.commit || input.tree != output.tree
                || output.content.allocations != input.allocations
                || output.file.source != (SnapshotFileSource::Container {
                    branch: input.base.branch_id(), node: FileNodeId::Version(material.version_id),
                })
                || version.version_id != material.version_id.to_string() || version.file_id != input.original.file_id.to_string()
                || version.logical_size != material.size || version.content_hash != material.content_hash
                || version.history_tree_id != input.history.to_string() || version.version_sequence != receipt.original.sequence
                || version.creation_name != output.path.file_name() || version.created_as_full_checkpoint
                || version.source_version_ids != [input.original.version_id.to_string()]
                || self.source.versions.get(&version.version_id) != Some(version)
                || self.source.file_node_parents.get(&version.version_id).is_none_or(|parents|
                    parents.as_slice() != [input.base.node_id().to_string()])
                || committed.operation_id != operation || committed.sequence != receipt.original.sequence
                || committed.tree_id != input.tree.to_string() || committed.history_tree_id != input.history.to_string()
                || committed.kind != "file_content" || committed.parent_commit_ids != [input.branch_base.to_string()]
                || committed.message != plan.request.message || committed.author != receipt.principal.as_str()
                || output.content.placement.route_slot_ids.as_slice() != [plan.host.registration.slot_id.clone()]
                || output.content.placement.binding_generations.get(&plan.host.registration.slot_id)
                    != Some(&plan.host.registration.binding_generation)
            { return Err(RegistrationError::Completion(operation)); }
            expected_protection.push(crate::persist::file_draft::content_protection_for_owner(input.owner, &output.content)
                .map_err(RegistrationError::Publication)?);
            let i = entries.binary_search_by(|e| e.path.key().cmp(input.edit.path.key()))
                .map_err(|_| RegistrationError::CommitScope)?;
            if entries[i].path != input.edit.path || entries[i].content != (SnapshotEntryContent::File {
                material: input.original, source: SnapshotFileSource::Container {
                    branch: input.base.branch_id(), node: input.base.node_id(),
                },
            }) { return Err(RegistrationError::CommitScope); }
            entries[i].content = SnapshotEntryContent::File { material, source: output.file.source };
        }
        // The native footer uses the order of actual Prepared versions. Those
        // are issued in original path order on this one producer.
        if expected_protection.as_slice() != protection || entries != value.prepared_node.entries
            || content_checkpoints(plan.before, &plan.events(&value.rows)).map_err(RegistrationError::Ledger)?
                != [value.committing, value.completed]
        { return Err(RegistrationError::Completion(operation)); }
        let manifest = AdmittedSnapshotManifest::prepare(plan.source.repository_id, plan.destination_node,
            operation, entries).map_err(RegistrationError::Manifest)?;
        if manifest.reference() != value.prepared_node.binding.reference() { return Err(RegistrationError::Source); }
        let node = NodePlan { request: source.plan.request.clone(), host: plan.host.clone(),
            allocated: NodeAllocation { repository: plan.source.repository_id, branch: plan.source.branch_id,
                node: plan.destination_node, completion: operation },
            branch_name: source.plan.branch_name.clone(), material: manifest.reference() };
        let op = value.reserve;
        self.branch_heads.insert(key, plan.handle());
        self.pending_branches.remove(&key);
        self.heads.insert(plan.request.target.worktree_id, LedgerHead { checkpoint: value.completed,
            revision: plan.request.expected });
        self.pending_edits.remove(&plan.request.target.worktree_id);
        self.frozen_content.retain(|_, owner| *owner != op);
        self.content_batches.get_mut(&op).ok_or(RegistrationError::Source)?.completion =
            Publication::Complete(Complete { value, manifest, node });
        Ok(())
    }

    pub(in crate::snapshot_worktree) fn registered_for_content(&self, grant: &FrozenContent<'_>)
        -> Result<super::PublishedWorktree<'_>>
    {
        let plan = grant.plan;
        let op = plan.request.operation_id;
        let entry = self.content_batches.get(&op).ok_or(RegistrationError::Source)?;
        let key = (plan.source.repository_id, plan.source.branch_id);
        if !std::ptr::eq(&entry.plan, plan) || entry.commitment != grant.commitment || entry.complete().is_some()
            || self.pending_branches.get(&key) != Some(&op) || self.branch_heads.get(&key) != Some(&plan.source)
            || self.pending_edits.get(&plan.request.target.worktree_id) != Some(&op)
        { return Err(RegistrationError::CommitScope); }
        self.worktree_for_commit(plan.request.target, Some(op))
    }
}
