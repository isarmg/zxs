//! A directory result is selected by its SYSTEM publication, never local HEAD.
use crate::directory_worktree::ledger::{Binding, Checkpoint, Event, LedgerError};
use crate::directory_worktree::lifecycle::{
    DirectoryDestination, Publication as LedgerPublication, PublicationIntent, PublicationMode,
    PublicationScope, SnapshotSource,
};
use crate::directory_worktree::projection::FileResult;
use crate::persist::snapshot_worktree::{
    CommitCompletion, CommitNode, CommitReservation, CommitRouting,
};
use crate::snapshot_worktree::registry::{
    Allocations, CompletionOwner, NodePlan, Publication, PublishedNode, Receipt, ReceiptIndex,
    require_completion,
};
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, HeldSnapshotNode, SnapshotEntryContent, SnapshotFileSource,
};
use crate::{CommitState, State};
use storage_contract::snapshot_worktree::{CommitWorktree, SnapshotHandle, WorktreeHandle};
use storage_contract::working_tree::MaterialCommitment;
use storage_contract::{FileNodeId, OperationId, Principal};

pub(super) struct CommitRecord<'a> {
    pub(super) receipt: Receipt<'a>,
    pub(super) plan: CommitReservation,
    pub(super) routing: CommitRouting,
    prepared_at: u64,
    pub(super) node_plan: NodePlan,
    pub(super) manifest: AdmittedSnapshotManifest,
    pub(super) file_result: FileResult,
    source_binding: crate::working_tree::snapshot::SnapshotNodeBinding,
    pub(super) commitment: MaterialCommitment,
    pub(super) node: Publication<CommitNode>,
    pub(super) completion: Publication<CommitCompletion>,
}

pub(in crate::snapshot_worktree) enum CommitRetry<'a> {
    Complete(SnapshotHandle),
    Published(PublishedCommitTail<'a>),
}

/// Only the admitted index can issue this after the exact FILE child and node
/// publication. It selects two deterministic ledger frames, not a local tail
/// or permission to construct a FILE version or adopt an unregistered node.
pub(crate) struct PublishedCommitTail<'a> {
    entry: &'a CommitRecord<'a>,
    node: crate::working_tree::snapshot::SnapshotNodeBinding,
}

impl PublishedCommitTail<'_> {
    pub(in crate::snapshot_worktree) fn plan(&self) -> &CommitReservation {
        &self.entry.plan
    }

    pub(in crate::snapshot_worktree) fn routing(&self) -> CommitRouting { self.entry.routing }

    pub(in crate::snapshot_worktree) fn commitment(&self) -> MaterialCommitment {
        self.entry.commitment
    }

    pub(crate) fn binding(&self) -> Binding {
        self.entry.plan.binding
    }

    pub(crate) fn before(&self) -> Checkpoint {
        self.entry.plan.before
    }

    pub(crate) fn transitions(&self) -> [Event; 2] {
        let plan = &self.entry.plan;
        [Event::Commit {
            operation: plan.request.operation_id,
            expected: plan.request.expected,
            current_head: self.entry.routing.observed_head(),
            fork_branch: self.entry.routing.directory.branch(),
        }, Event::Completed {
            publication: plan.publication_at(self.entry.routing),
            files: vec![self.entry.file_result.clone()],
        }]
    }

    pub(crate) fn require_nodes(&self, source: &HeldSnapshotNode, node: &HeldSnapshotNode)
        -> std::result::Result<(), LedgerError>
    {
        if source.binding() != self.entry.source_binding || node.binding() != self.node {
            return Err(LedgerError::Binding);
        }
        Ok(())
    }

    pub(crate) fn require_publication(&self, publication: &LedgerPublication, files: &[FileResult])
        -> std::result::Result<(), LedgerError>
    {
        self.entry.require_publication(publication, files)
    }
}

impl CommitRecord<'_> {
    fn require_publication(&self, publication: &LedgerPublication, files: &[FileResult])
        -> std::result::Result<(), LedgerError>
    {
        if !matches!(self.node, Publication::Complete(_))
            || *publication != self.plan.publication_at(self.routing)
            || files != std::slice::from_ref(&self.file_result)
        {
            return Err(LedgerError::FileResults);
        }
        Ok(())
    }
}

impl CommitRouting {
    pub(in crate::snapshot_worktree) fn observed_head(self)
        -> storage_contract::working_tree::SnapshotNodeId
    {
        match self.directory {
            DirectoryDestination::Continue { expected_head, .. } => expected_head,
            DirectoryDestination::Fork { expected_source_head, .. } => expected_source_head,
        }
    }
}

impl CommitReservation {
    pub(in crate::snapshot_worktree) fn handle(&self) -> SnapshotHandle {
        self.handle_at(self.source.branch_id)
    }

    pub(in crate::snapshot_worktree) fn handle_at(
        &self, branch: storage_contract::working_tree::SnapshotBranchId,
    ) -> SnapshotHandle {
        SnapshotHandle {
            publication: self.node_publication,
            repository_id: self.source.repository_id,
            branch_id: branch,
            node_id: self.node,
        }
    }

    pub(in crate::snapshot_worktree) fn file_result(&self) -> Result<FileResult> {
        let SnapshotFileSource::Container { branch, .. } = self.original_source else {
            return Err(RegistrationError::CommitScope);
        };
        Ok(self.file_result_at(branch))
    }

    pub(in crate::snapshot_worktree) fn file_result_at(&self, branch: storage_contract::BranchId)
        -> FileResult
    {
        FileResult {
            path: self.edit.path.clone(), material: self.edit.material,
            source: SnapshotFileSource::Container {
                branch, node: FileNodeId::Version(self.edit.material.version_id),
            },
        }
    }

    pub(in crate::snapshot_worktree) fn publication_at(&self, routing: CommitRouting)
        -> LedgerPublication
    {
        LedgerPublication {
            intent: PublicationIntent {
                worktree: self.request.target.worktree_id,
                source: SnapshotSource {
                    repository: self.source.repository_id,
                    node: self.source.node_id,
                    branch: self.source.branch_id,
                    manifest: self.binding.source().manifest,
                },
                revision: self.request.expected,
                operation: self.request.operation_id,
                mode: PublicationMode::DirectoryCommit {
                    destination: routing.directory,
                },
            },
            scope: PublicationScope::Directory {
                branch: routing.directory.branch(), node: self.node,
            },
        }
    }
}

impl<'a> ReceiptIndex<'a> {
    pub(in crate::snapshot_worktree) fn into_commit_manifest(mut self, reserve: OperationId)
        -> Result<AdmittedSnapshotManifest>
    {
        let entry = self.commits.remove(&reserve).ok_or(RegistrationError::CommitScope)?;
        Ok(entry.manifest)
    }

    pub(in crate::snapshot_worktree) fn require_committed_publication(
        &self, reserve: OperationId, publication: &LedgerPublication, files: &[FileResult],
    ) -> std::result::Result<(), LedgerError> {
        let entry = self.commits.get(&reserve).ok_or(LedgerError::PublicationPending)?;
        entry.require_publication(publication, files)
    }

    pub(in crate::snapshot_worktree) fn retry_commit(
        &self, request: &CommitWorktree, principal: &Principal,
    ) -> Result<Option<CommitRetry<'_>>> {
        if !self.source.operations.contains_key(&request.operation_id) {
            return Ok(None);
        }
        if let Some(original) = self.historical.get(&request.operation_id) {
            if &original.plan.request != request || &original.receipt.principal != principal {
                return Err(RegistrationError::Digest(request.operation_id));
            }
            if !self.commits.contains_key(&request.operation_id) {
                return Err(RegistrationError::Pending(request.operation_id));
            }
        }
        let entry = self.commits.get(&request.operation_id)
            .ok_or(RegistrationError::Operation(request.operation_id))?;
        if &entry.plan.request != request || &entry.receipt.principal != principal {
            return Err(RegistrationError::Digest(request.operation_id));
        }
        if matches!(entry.completion, Publication::Complete(_)) {
            return Ok(Some(CommitRetry::Complete(
                entry.plan.handle_at(entry.routing.directory.branch()),
            )));
        }
        if let Publication::Complete(node) = &entry.node {
            return Ok(Some(CommitRetry::Published(PublishedCommitTail {
                entry, node: node.binding,
            })));
        }
        Err(RegistrationError::Pending(request.operation_id))
    }

    pub(in crate::snapshot_worktree) fn completed_edit(
        &self, target: WorktreeHandle,
    ) -> Result<(&crate::persist::snapshot_worktree::EditReservation,
        &crate::persist::snapshot_worktree::EditCompletion)>
    {
        self.completed_edit_for(target, None)
    }

    fn completed_edit_for(
        &self, target: WorktreeHandle, owner: Option<OperationId>,
    ) -> Result<(&crate::persist::snapshot_worktree::EditReservation,
        &crate::persist::snapshot_worktree::EditCompletion)>
    {
        let revision = self.worktree_for_commit(target, owner)?.revision();
        // The independent single-file/fork path cannot publish just the last
        // save from a multiple-occurrence worktree.
        let [edit] = self.active_edits(target) else {
            return Err(RegistrationError::CommitScope);
        };
        let current = self.original_edit(target, edit)?;
        if current.1.revision != revision {
            return Err(RegistrationError::CommitScope);
        }
        Ok(current)
    }

    pub(super) fn committed_node(&self, reserve: OperationId, handle: SnapshotHandle)
        -> Result<PublishedNode<'_>>
    {
        let entry = self.commits.get(&reserve).ok_or(RegistrationError::Source)?;
        if handle != entry.plan.handle_at(entry.routing.directory.branch()) {
            return Err(RegistrationError::Source);
        }
        let Publication::Complete(completed) = &entry.node else {
            return Err(RegistrationError::Pending(reserve));
        };
        Ok(PublishedNode {
            plan: &entry.node_plan, manifest: &entry.manifest, binding: completed.binding,
        })
    }

    pub(super) fn reserve_commit(
        &mut self, receipt: Receipt<'a>, plan: CommitReservation,
        commitment: MaterialCommitment, allocations: &mut Allocations,
    ) -> Result<()> {
        let SnapshotFileSource::Container { branch, .. } = plan.original_source else {
            return Err(RegistrationError::CommitScope);
        };
        let name = self.node(plan.source)?.branch_name().clone();
        let routing = CommitRouting {
            directory: DirectoryDestination::Continue {
                branch: plan.source.branch_id, expected_head: plan.source.node_id,
            },
            file_branch: branch,
        };
        let prepared_at = receipt.original.sequence;
        self.admit_commit(receipt, plan, commitment, allocations, routing, name, prepared_at, false)
    }

    pub(super) fn admit_commit(
        &mut self, receipt: Receipt<'a>, plan: CommitReservation,
        commitment: MaterialCommitment, allocations: &mut Allocations,
        routing: CommitRouting, branch_name: storage_contract::BranchName,
        prepared_at: u64, historical: bool,
    ) -> Result<()> {
        let operation = receipt.original.operation_id;
        if plan.request.operation_id != operation {
            return Err(RegistrationError::Operation(operation));
        }
        let owned_pending = historical.then_some(operation);
        let (edit, completed) = self.completed_edit_for(plan.request.target, owned_pending)?;
        let worktree = self.worktree_for_commit(plan.request.target, owned_pending)?;
        if plan.source != worktree.source().handle() || plan.binding != worktree.binding()
            || plan.before != worktree.checkpoint() || plan.edit != completed.edit
            || plan.original != edit.original || plan.original_source != edit.original_source
            || plan.request.expected != completed.revision
            || plan.request.expected != worktree.revision()
            || self.branch_heads.get(&(plan.source.repository_id, plan.source.branch_id))
                .map(|head| head.node_id) != Some(routing.observed_head())
            || self.pending_branches.get(&(plan.source.repository_id, plan.source.branch_id))
                .copied() != owned_pending
            || self.pending_branches.get(&(plan.source.repository_id, routing.directory.branch()))
                .copied() != owned_pending
        {
            return Err(RegistrationError::CommitScope);
        }
        let SnapshotFileSource::Container { node, .. } = plan.original_source else {
            return Err(RegistrationError::CommitScope);
        };
        let file = self.source.versions.get(&plan.original.version_id.to_string())
            .ok_or(RegistrationError::CommitScope)?;
        let branch_record = self.source.branches.get(&routing.file_branch.to_string())
            .ok_or(RegistrationError::CommitScope)?;
        let base = self.source.commits.get(&plan.file_base.to_string())
            .ok_or(RegistrationError::CommitScope)?;
        if file.history_tree_id != plan.file_history.to_string()
            || file.file_id != plan.original.file_id.to_string()
            || file.logical_size != plan.original.size
            || file.content_hash != plan.original.content_hash
            || branch_record.file_id.as_deref() != Some(file.file_id.as_str())
            || branch_record.history_tree_id != file.history_tree_id
            || base.history_tree_id != file.history_tree_id
            || base.sequence > prepared_at || base.entries.len() != 1
            || base.entries[0].file_id != file.file_id
            || base.entries[0].version_id != file.version_id
            || crate::business::file_nodes::node_version(
                self.source, &file.file_id, &node.to_string(),
            ).map_err(RegistrationError::Publication)? != file.version_id
        {
            return Err(RegistrationError::CommitScope);
        }
        let source = worktree.source();
        let source_binding = source.binding();
        let mut entries = source.manifest.entries().to_vec();
        let file_result = plan.file_result_at(routing.file_branch);
        let entry = entries.iter_mut().find(|entry| entry.path == file_result.path)
            .ok_or(RegistrationError::CommitScope)?;
        entry.content = SnapshotEntryContent::File {
            material: file_result.material, source: file_result.source,
        };
        let manifest = AdmittedSnapshotManifest::prepare(
            plan.source.repository_id, plan.node, operation, entries,
        ).map_err(RegistrationError::Manifest)?;
        let node_plan = NodePlan {
            request: source.plan.request.clone(), host: source.plan.host.clone(),
            allocated: crate::persist::snapshot_worktree::NodeAllocation {
                repository: plan.source.repository_id, branch: routing.directory.branch(),
                node: plan.node, completion: plan.node_publication,
            },
            branch_name, material: manifest.reference(),
        };
        if !historical && (!allocations.nodes.insert(plan.node)
            || plan.file_operation == operation || plan.file_operation == plan.node_publication
            || plan.file_operation == plan.completion
            || self.completions.contains_key(&plan.file_operation)
            || !self.planned_files.insert(plan.file_operation)
            || self.source.operations.get(&plan.file_operation)
                .is_some_and(|file| file.sequence <= receipt.original.sequence))
        {
            return Err(RegistrationError::Identity);
        }
        if !historical {
            self.reserve_completion(
                operation, plan.node_publication, CompletionOwner::CommitNode(operation),
            )?;
            self.reserve_completion(operation, plan.completion, CompletionOwner::Commit)?;
        }
        self.pending_edits.insert(plan.request.target.worktree_id, operation);
        self.pending_branches.insert((plan.source.repository_id, plan.source.branch_id), operation);
        self.commits.insert(operation, CommitRecord {
            receipt, plan, routing, prepared_at, node_plan, manifest, file_result,
            source_binding, commitment,
            node: Publication::Reserved, completion: Publication::Reserved,
        });
        Ok(())
    }

    pub(super) fn publish_commit_node(&mut self, receipt: Receipt<'a>, completed: CommitNode)
        -> Result<()>
    {
        let operation = receipt.original.operation_id;
        let entry = self.commits.get_mut(&completed.reserve)
            .ok_or(RegistrationError::Completion(operation))?;
        require_completion(&entry.receipt, &receipt, entry.plan.node_publication,
            entry.commitment, completed.reservation)?;
        verify_file_publication(self.source, &entry.plan, entry.routing.file_branch,
            &entry.receipt.principal, entry.prepared_at, receipt.original.sequence)?;
        if !matches!(entry.node, Publication::Reserved)
            || completed.binding.reference() != entry.manifest.reference()
            || !completed.binding.same_repository(entry.source_binding)
        {
            return Err(RegistrationError::Completion(operation));
        }
        self.branch_heads.insert(
            (entry.plan.source.repository_id, entry.routing.directory.branch()),
            entry.plan.handle_at(entry.routing.directory.branch()),
        );
        entry.node = Publication::Complete(completed);
        Ok(())
    }

    pub(super) fn complete_commit(&mut self, receipt: Receipt<'a>, completed: CommitCompletion)
        -> Result<()>
    {
        let operation = receipt.original.operation_id;
        let entry = self.commits.get_mut(&completed.reserve)
            .ok_or(RegistrationError::Completion(operation))?;
        require_completion(&entry.receipt, &receipt, entry.plan.completion,
            entry.commitment, completed.reservation)?;
        let node_receipt = self.source.operations.get(&entry.plan.node_publication)
            .ok_or(RegistrationError::Completion(operation))?;
        let next = entry.plan.before.sequence.next().and_then(|sequence| sequence.next())
            .map_err(|source| RegistrationError::Ledger(LedgerError::Sequence(source)))?;
        if !matches!(entry.node, Publication::Complete(_))
            || !matches!(entry.completion, Publication::Reserved)
            || node_receipt.sequence >= receipt.original.sequence
            || completed.checkpoint.sequence != next
        {
            return Err(RegistrationError::Completion(operation));
        }
        let worktree = entry.plan.request.target.worktree_id;
        self.heads.insert(worktree, crate::snapshot_worktree::registry::edits::LedgerHead {
            checkpoint: completed.checkpoint, revision: entry.plan.request.expected,
        });
        self.pending_edits.remove(&worktree);
        self.pending_branches.remove(
            &(entry.plan.source.repository_id, entry.plan.source.branch_id),
        );
        self.pending_branches.remove(
            &(entry.plan.source.repository_id, entry.routing.directory.branch()),
        );
        entry.completion = Publication::Complete(completed);
        Ok(())
    }

    pub(super) fn worktree_publication(&self, worktree: WorktreeHandle)
        -> Option<&CommitRecord<'_>>
    {
        self.commits.values().find(|entry| entry.plan.request.target == worktree
            && matches!(entry.completion, Publication::Complete(_)))
    }
}

/// The actual ordinary FILE publication must precede the new node. Neither a
/// matching BODY nor an existing version with another operation can discharge it.
pub(in crate::snapshot_worktree) fn verify_file_publication(
    state: &State, plan: &CommitReservation, branch: storage_contract::BranchId,
    principal: &Principal,
    reserved_at: u64, before: u64,
) -> Result<()> {
    let receipt = state.operations.get(&plan.file_operation)
        .ok_or(RegistrationError::CommitScope)?;
    let commit = receipt.commit_id.as_ref().and_then(|commit| state.commits.get(commit))
        .ok_or(RegistrationError::CommitScope)?;
    let file = state.versions.get(&plan.edit.material.version_id.to_string())
        .ok_or(RegistrationError::CommitScope)?;
    if receipt.kind != "workspace.commit" || receipt.commit_state != CommitState::Published
        || receipt.sequence <= reserved_at || receipt.sequence >= before
        || !plan.file_request.matches(&receipt.request_digest)
        || state.operation_principals.get(&plan.file_operation).map(String::as_str)
            != Some(principal.as_str())
        || receipt.branch_id.as_deref() != Some(branch.to_string().as_str())
        || receipt.history_tree_id.as_deref() != Some(plan.file_history.to_string().as_str())
        || commit.operation_id != plan.file_operation || commit.sequence != receipt.sequence
        || commit.parent_commit_ids != [plan.file_base.to_string()]
        || commit.entries.len() != 1
        || commit.entries[0].file_id != plan.edit.material.file_id.to_string()
        || commit.entries[0].version_id != plan.edit.material.version_id.to_string()
        || file.file_id != plan.original.file_id.to_string()
        || file.history_tree_id != plan.file_history.to_string()
        || file.logical_size != plan.edit.material.size
        || file.content_hash != plan.edit.material.content_hash
        || file.version_sequence != receipt.sequence
        || file.source_version_ids != [plan.original.version_id.to_string()]
    {
        return Err(RegistrationError::CommitScope);
    }
    Ok(())
}
