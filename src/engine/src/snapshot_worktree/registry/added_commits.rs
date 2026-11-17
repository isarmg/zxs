//! The original Added commit, its exact native FILE child, and independent node.
use super::{
    Allocations, CompletionOwner, NodePlan, Publication, PublishedNode, Receipt, ReceiptIndex,
    require_completion,
};
use crate::directory_worktree::ledger::AddedFileResult;
use crate::directory_worktree::ledger::{Binding, Checkpoint, Event, LedgerError};
use crate::directory_worktree::lifecycle::{
    Publication as LedgerPublication, PublicationIntent, PublicationMode, PublicationScope,
    SnapshotSource,
};
use crate::persist::snapshot_worktree::added_commit::{AddedCommit, AddedFirst};
use crate::persist::snapshot_worktree::{CommitCompletion, CommitNode, Record};
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, HeldSnapshotNode, SnapshotEntry, SnapshotEntryContent,
    SnapshotFileSource, SnapshotNodeBinding,
};
use storage_contract::snapshot_worktree::{CommitWorktree, SnapshotHandle, WorktreeHandle};
use storage_contract::working_tree::MaterialCommitment;
use storage_contract::{FileNodeId, OperationId, Principal};

pub(super) struct FirstPublication {
    pub(super) first: AddedFirst,
    pub(super) result: AddedFileResult,
    pub(super) manifest: AdmittedSnapshotManifest,
    pub(super) node_plan: NodePlan,
}
pub(super) struct AddedCommitRecord<'a> {
    pub(super) receipt: Receipt<'a>,
    pub(super) plan: AddedCommit,
    pub(super) commitment: MaterialCommitment,
    source_binding: SnapshotNodeBinding,
    pub(super) first: Publication<FirstPublication>,
    pub(super) node: Publication<CommitNode>,
    pub(super) completion: Publication<CommitCompletion>,
}
impl AddedCommitRecord<'_> {
    fn first(&self) -> Result<&FirstPublication> {
        match &self.first {
            Publication::Complete(first) => Ok(first),
            Publication::Reserved => {
                Err(RegistrationError::Pending(self.plan.request.operation_id))
            }
        }
    }
    pub(super) fn require_publication(
        &self,
        publication: &LedgerPublication,
        file: &AddedFileResult,
    ) -> std::result::Result<(), LedgerError> {
        let Publication::Complete(first) = &self.first else {
            return Err(LedgerError::PublicationPending);
        };
        if !matches!(self.node, Publication::Complete(_))
            || *publication != self.plan.publication()
            || *file != first.result
        {
            return Err(LedgerError::FileResults);
        }
        Ok(())
    }
}
impl AddedCommit {
    pub(crate) fn publication(&self) -> LedgerPublication {
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
                    destination: self.routing().directory,
                },
            },
            scope: PublicationScope::Directory {
                branch: self.destination,
                node: self.node,
            },
        }
    }
    pub(crate) fn commit_event(&self) -> Event {
        Event::CommitAdded {
            operation: self.request.operation_id,
            expected: self.request.expected,
            current_head: self.observed_head.node_id,
            fork_branch: self.destination,
        }
    }
}
impl AddedFirst {
    pub(crate) fn file_result(&self, plan: &AddedCommit) -> Result<AddedFileResult> {
        let material = self.material().map_err(RegistrationError::Allocation)?;
        Ok(AddedFileResult {
            target: plan.added.target.clone(),
            birth: plan.added.birth,
            working: plan.added.working.clone(),
            file_operation: plan.file_operation,
            material,
            source: SnapshotFileSource::Container {
                branch: plan.file_branch,
                node: FileNodeId::Version(material.version_id),
            },
        })
    }
}

pub(crate) struct PublishedAddedCommitTail<'a> {
    entry: &'a AddedCommitRecord<'a>,
    first: &'a FirstPublication,
    node: SnapshotNodeBinding,
}
impl PublishedAddedCommitTail<'_> {
    pub(in crate::snapshot_worktree) fn plan(&self) -> &AddedCommit {
        &self.entry.plan
    }
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
        [
            self.entry.plan.commit_event(),
            Event::AddedCompleted {
                publication: self.entry.plan.publication(),
                file: self.first.result.clone(),
            },
        ]
    }
    pub(crate) fn require_nodes(
        &self,
        source: &HeldSnapshotNode,
        node: &HeldSnapshotNode,
    ) -> std::result::Result<(), LedgerError> {
        if source.binding() != self.entry.source_binding || node.binding() != self.node {
            return Err(LedgerError::Binding);
        }
        Ok(())
    }
    pub(crate) fn require_publication(
        &self,
        publication: &LedgerPublication,
        file: &AddedFileResult,
    ) -> std::result::Result<(), LedgerError> {
        self.entry.require_publication(publication, file)
    }
}
pub(in crate::snapshot_worktree) enum AddedCommitRetry<'a> {
    Complete(SnapshotHandle),
    Published(PublishedAddedCommitTail<'a>),
}

impl<'a> ReceiptIndex<'a> {
    pub(crate) fn require_added_first(
        &self,
        plan: &AddedCommit,
        principal: &Principal,
    ) -> Result<()> {
        let record = self
            .added_commits
            .get(&plan.request.operation_id)
            .ok_or(RegistrationError::Source)?;
        if &record.plan != plan
            || &record.receipt.principal != principal
            || !matches!(record.first, Publication::Reserved)
            || self.pending_edits.get(&plan.request.target.worktree_id)
                != Some(&plan.request.operation_id)
        {
            return Err(RegistrationError::CommitScope);
        }
        Ok(())
    }
    pub(in crate::snapshot_worktree) fn retry_added_commit(
        &self,
        request: &CommitWorktree,
        principal: &Principal,
    ) -> Result<Option<AddedCommitRetry<'_>>> {
        let Some(entry) = self.added_commits.get(&request.operation_id) else {
            return Ok(None);
        };
        if &entry.plan.request != request || &entry.receipt.principal != principal {
            return Err(RegistrationError::Digest(request.operation_id));
        }
        if matches!(entry.completion, Publication::Complete(_)) {
            return Ok(Some(AddedCommitRetry::Complete(entry.plan.handle())));
        }
        if let Publication::Complete(node) = &entry.node {
            return Ok(Some(AddedCommitRetry::Published(
                PublishedAddedCommitTail {
                    entry,
                    first: entry.first()?,
                    node: node.binding,
                },
            )));
        }
        Err(RegistrationError::Pending(request.operation_id))
    }
    pub(super) fn reserve_added_commit(
        &mut self,
        receipt: Receipt<'a>,
        plan: AddedCommit,
        commitment: MaterialCommitment,
        allocations: &mut Allocations,
    ) -> Result<()> {
        let operation = receipt.original.operation_id;
        let current = self.worktree(plan.request.target)?;
        let source = current.source();
        if plan.request.operation_id != operation
            || current.published_node().is_some()
            || plan.source != source.handle()
            || plan.binding != current.binding()
            || plan.before != current.checkpoint()
            || plan.request.expected != current.revision()
            || self.added(plan.request.target) != std::slice::from_ref(&plan.added)
            || self
                .active_file_edits
                .get(&plan.request.target.worktree_id)
                .is_some_and(|edits| !edits.is_empty())
            || self.commit_head(plan.source)? != plan.observed_head
        {
            return Err(RegistrationError::CommitScope);
        }
        let container = self
            .source
            .file_containers
            .get(&plan.added.target.file)
            .ok_or(RegistrationError::CommitScope)?;
        if container.created_by != plan.added.birth
            || container.history_tree_id != plan.history
            || container.namespace.repository != source.host().namespace.repository_id
            || plan.added.target.path.parent().is_some()
        {
            return Err(RegistrationError::CommitScope);
        }
        let source_binding = source.binding();
        if plan.source == plan.observed_head {
            if plan.destination != plan.source.branch_id
                || &plan.branch_name != source.branch_name()
            {
                return Err(RegistrationError::CommitScope);
            }
        } else {
            if plan.destination == plan.source.branch_id
                || plan.branch_name.as_str() != current.reservation().request.name.as_str()
                || !allocations.branches.insert(plan.destination)
                || self.nodes.values().any(|node| {
                    node.plan.allocated.repository == plan.source.repository_id
                        && node.plan.branch_name.key() == plan.branch_name.key()
                })
                || self.commits.values().any(|node| {
                    node.plan.source.repository_id == plan.source.repository_id
                        && node.node_plan.branch_name.key() == plan.branch_name.key()
                })
                || self.historical.values().any(|node| {
                    node.plan.source.repository_id == plan.source.repository_id
                        && node.plan.branch_name.key() == plan.branch_name.key()
                })
                || self.added_commits.values().any(|node| {
                    node.plan.source.repository_id == plan.source.repository_id
                        && node.plan.branch_name.key() == plan.branch_name.key()
                })
            {
                return Err(RegistrationError::Identity);
            }
        }
        if !allocations.nodes.insert(plan.node)
            || self.added_commits.values().any(|entry| {
                entry.plan.added.target.file == plan.added.target.file
                    || entry.plan.file_branch == plan.file_branch
                    || entry.plan.file_commit == plan.file_commit
                    || entry.plan.file_tree == plan.file_tree
            })
            || self
                .source
                .branches
                .get(&plan.file_branch.to_string())
                .is_some_and(|branch| {
                    self.source
                        .commits
                        .get(&branch.fork_commit_id)
                        .is_some_and(|commit| commit.sequence > receipt.original.sequence)
                        || branch.file_id.as_deref()
                            != Some(plan.added.target.file.to_string().as_str())
                })
            || self
                .source
                .commits
                .get(&plan.file_commit.to_string())
                .is_some_and(|commit| commit.sequence <= receipt.original.sequence)
        {
            return Err(RegistrationError::Identity);
        }
        self.reserve_completion(operation, plan.file_operation, CompletionOwner::AddedFirst)?;
        self.reserve_completion(
            operation,
            plan.node_publication,
            CompletionOwner::AddedNode(operation),
        )?;
        self.reserve_completion(operation, plan.completion, CompletionOwner::AddedCommit)?;
        self.pending_edits
            .insert(plan.request.target.worktree_id, operation);
        self.pending_branches.insert(
            (plan.source.repository_id, plan.source.branch_id),
            operation,
        );
        self.pending_branches
            .insert((plan.source.repository_id, plan.destination), operation);
        self.added_commits.insert(
            operation,
            AddedCommitRecord {
                receipt,
                plan,
                commitment,
                source_binding,
                first: Publication::Reserved,
                node: Publication::Reserved,
                completion: Publication::Reserved,
            },
        );
        Ok(())
    }
    pub(super) fn publish_added_file(
        &mut self,
        receipt: Receipt<'a>,
        first: AddedFirst,
    ) -> Result<()> {
        let entry = self
            .added_commits
            .get(&first.reserve)
            .ok_or(RegistrationError::Source)?;
        require_completion(
            &entry.receipt,
            &receipt,
            entry.plan.file_operation,
            entry.commitment,
            first.reservation,
        )?;
        if !matches!(entry.first, Publication::Reserved) || first.full.entity.inode() == 0 {
            return Err(RegistrationError::Completion(receipt.original.operation_id));
        }
        let plan = &entry.plan;
        let version = self
            .source
            .versions
            .get(&first.content.version.version_id)
            .ok_or(RegistrationError::CommitScope)?;
        let branch = self
            .source
            .branches
            .get(&plan.file_branch.to_string())
            .ok_or(RegistrationError::CommitScope)?;
        let commit = self
            .source
            .commits
            .get(&plan.file_commit.to_string())
            .ok_or(RegistrationError::CommitScope)?;
        let container = self
            .source
            .file_containers
            .get(&plan.added.target.file)
            .ok_or(RegistrationError::CommitScope)?;
        if version != &first.content.version
            || version.file_id != plan.added.target.file.to_string()
            || version.history_tree_id != plan.history.to_string()
            || version.logical_size != plan.added.working.logical_size()
            || version.version_sequence != receipt.original.sequence
            || !version.source_version_ids.is_empty()
            || !first.content.allocations.is_empty()
            || self.source.placements.get(&version.version_id) != Some(&first.content.placement)
            || first.content.placement.route_slot_ids.len() != 1
            || first.content.placement.target_copies != 1
            || branch.file_id.as_deref() != Some(version.file_id.as_str())
            || branch.history_tree_id != version.history_tree_id
            || commit.operation_id != receipt.original.operation_id
            || commit.sequence != receipt.original.sequence
            || commit.entries.len() != 1
            || commit.entries[0].file_id != version.file_id
            || commit.entries[0].entry_id != plan.added.target.entry.to_string()
            || commit.entries[0].path != plan.added.target.path
            || commit.entries[0].version_id != version.version_id
            || container.namespace.entry != plan.added.target.entry
            || container.created_by != plan.added.birth
        {
            return Err(RegistrationError::CommitScope);
        }
        let source = self.node(plan.source)?;
        let result = first.file_result(plan)?;
        let mut entries = source.manifest().entries().to_vec();
        entries.push(SnapshotEntry {
            path: result.target.path.clone(),
            content: SnapshotEntryContent::File {
                material: result.material,
                source: result.source,
            },
        });
        entries.sort_by(|left, right| left.path.key().cmp(right.path.key()));
        let manifest = AdmittedSnapshotManifest::prepare(
            plan.source.repository_id,
            plan.node,
            plan.request.operation_id,
            entries,
        )
        .map_err(RegistrationError::Manifest)?;
        let node_plan = NodePlan {
            request: source.plan.request.clone(),
            host: source.plan.host.clone(),
            allocated: crate::persist::snapshot_worktree::NodeAllocation {
                repository: plan.source.repository_id,
                branch: plan.destination,
                node: plan.node,
                completion: plan.node_publication,
            },
            branch_name: plan.branch_name.clone(),
            material: manifest.reference(),
        };
        let reservation = first.reserve;
        self.added_commits
            .get_mut(&reservation)
            .ok_or(RegistrationError::Source)?
            .first = Publication::Complete(FirstPublication {
            first,
            result,
            manifest,
            node_plan,
        });
        Ok(())
    }
    pub(super) fn publish_added_node(
        &mut self,
        receipt: Receipt<'a>,
        node: CommitNode,
    ) -> Result<()> {
        let entry = self
            .added_commits
            .get_mut(&node.reserve)
            .ok_or(RegistrationError::Source)?;
        require_completion(
            &entry.receipt,
            &receipt,
            entry.plan.node_publication,
            entry.commitment,
            node.reservation,
        )?;
        let first = entry.first()?;
        let file_receipt = self
            .source
            .operations
            .get(&entry.plan.file_operation)
            .ok_or(RegistrationError::Source)?;
        if !matches!(entry.node, Publication::Reserved)
            || file_receipt.sequence >= receipt.original.sequence
            || node.binding.reference() != first.manifest.reference()
            || !node.binding.same_repository(entry.source_binding)
        {
            return Err(RegistrationError::Completion(receipt.original.operation_id));
        }
        self.branch_heads.insert(
            (entry.plan.source.repository_id, entry.plan.destination),
            entry.plan.handle(),
        );
        entry.node = Publication::Complete(node);
        Ok(())
    }
    pub(super) fn complete_added_commit(
        &mut self,
        receipt: Receipt<'a>,
        complete: CommitCompletion,
    ) -> Result<()> {
        let entry = self
            .added_commits
            .get_mut(&complete.reserve)
            .ok_or(RegistrationError::Source)?;
        require_completion(
            &entry.receipt,
            &receipt,
            entry.plan.completion,
            entry.commitment,
            complete.reservation,
        )?;
        let node = self
            .source
            .operations
            .get(&entry.plan.node_publication)
            .ok_or(RegistrationError::Source)?;
        let next = entry
            .plan
            .before
            .sequence
            .next()
            .and_then(|sequence| sequence.next())
            .map_err(|source| RegistrationError::Ledger(LedgerError::Sequence(source)))?;
        if !matches!(entry.node, Publication::Complete(_))
            || !matches!(entry.completion, Publication::Reserved)
            || node.sequence >= receipt.original.sequence
            || complete.checkpoint.sequence != next
        {
            return Err(RegistrationError::Completion(receipt.original.operation_id));
        }
        self.heads.insert(
            entry.plan.request.target.worktree_id,
            super::edits::LedgerHead {
                checkpoint: complete.checkpoint,
                revision: entry.plan.request.expected,
            },
        );
        self.pending_edits
            .remove(&entry.plan.request.target.worktree_id);
        self.pending_branches
            .remove(&(entry.plan.source.repository_id, entry.plan.source.branch_id));
        self.pending_branches
            .remove(&(entry.plan.source.repository_id, entry.plan.destination));
        entry.completion = Publication::Complete(complete);
        Ok(())
    }
    pub(super) fn added_node(
        &self,
        reserve: OperationId,
        handle: SnapshotHandle,
    ) -> Result<PublishedNode<'_>> {
        let entry = self
            .added_commits
            .get(&reserve)
            .ok_or(RegistrationError::Source)?;
        if handle != entry.plan.handle() {
            return Err(RegistrationError::Source);
        }
        let Publication::Complete(node) = &entry.node else {
            return Err(RegistrationError::Pending(reserve));
        };
        let first = entry.first()?;
        Ok(PublishedNode {
            plan: &first.node_plan,
            manifest: &first.manifest,
            binding: node.binding,
        })
    }
    pub(in crate::snapshot_worktree) fn added_material(
        &self,
        operation: OperationId,
    ) -> Result<(AdmittedSnapshotManifest, AddedFirst, AddedFileResult)> {
        let first = self
            .added_commits
            .get(&operation)
            .ok_or(RegistrationError::Source)?
            .first()?;
        Ok((
            first.manifest.clone(),
            first.first.clone(),
            first.result.clone(),
        ))
    }
    pub(in crate::snapshot_worktree) fn require_added_publication(
        &self,
        operation: OperationId,
        publication: &LedgerPublication,
        file: &AddedFileResult,
    ) -> std::result::Result<(), LedgerError> {
        self.added_commits
            .get(&operation)
            .ok_or(LedgerError::PublicationPending)?
            .require_publication(publication, file)
    }
    pub(super) fn worktree_added_publication(
        &self,
        worktree: WorktreeHandle,
    ) -> Option<&AddedCommitRecord<'_>> {
        self.added_commits.values().find(|entry| {
            entry.plan.request.target == worktree
                && matches!(entry.completion, Publication::Complete(_))
        })
    }
}
