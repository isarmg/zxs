//! Historical originals own their pending worktree, branch and child operations.
//! Only the exact ready receipt can bind the later ordinary FILE command.
use super::{Allocations, CompletionOwner, Receipt, ReceiptIndex, require_completion};
use crate::persist::snapshot_worktree::{
    CommitRouting, HistoricalCommit, HistoricalFile, HistoricalReady,
};
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::working_tree::snapshot::SnapshotFileSource;
use storage_contract::OperationId;
use storage_contract::snapshot_worktree::SnapshotHandle;
use storage_contract::working_tree::MaterialCommitment;

pub(super) struct HistoricalRecord<'a> {
    pub(super) receipt: Receipt<'a>,
    pub(super) plan: HistoricalCommit,
    pub(super) commitment: MaterialCommitment,
}

impl<'a> ReceiptIndex<'a> {
    pub(in crate::snapshot_worktree) fn commit_head(
        &self,
        source: SnapshotHandle,
    ) -> Result<SnapshotHandle> {
        self.node(source)?;
        let key = (source.repository_id, source.branch_id);
        if let Some(operation) = self.pending_branches.get(&key) {
            return Err(RegistrationError::Pending(*operation));
        }
        self.branch_heads
            .get(&key)
            .copied()
            .ok_or(RegistrationError::Source)
    }

    pub(super) fn reserve_historical(
        &mut self,
        receipt: Receipt<'a>,
        plan: HistoricalCommit,
        commitment: MaterialCommitment,
        allocations: &mut Allocations,
    ) -> Result<()> {
        let operation = receipt.original.operation_id;
        if plan.request.operation_id != operation || self.historical.contains_key(&operation) {
            return Err(RegistrationError::Operation(operation));
        }
        let (edited, completed) = self.completed_edit(plan.request.target)?;
        let worktree = self.worktree(plan.request.target)?;
        let source = worktree.source();
        if plan.source != source.handle()
            || plan.binding != worktree.binding()
            || plan.before != worktree.checkpoint()
            || plan.edit != completed.edit
            || plan.original != edited.original
            || plan.original_source != edited.original_source
            || plan.request.expected != completed.revision
            || plan.request.expected != worktree.revision()
            || self.commit_head(plan.source)? != plan.observed_head
        {
            return Err(RegistrationError::CommitScope);
        }
        let SnapshotFileSource::Container { branch, node } = plan.original_source else {
            return Err(RegistrationError::CommitScope);
        };
        let file = self
            .source
            .versions
            .get(&plan.original.version_id.to_string())
            .ok_or(RegistrationError::CommitScope)?;
        if file.file_id != plan.original.file_id.to_string()
            || file.history_tree_id != plan.file_history.to_string()
            || file.logical_size != plan.original.size
            || file.content_hash != plan.original.content_hash
            || crate::business::file_nodes::node_version(
                self.source,
                &file.file_id,
                &node.to_string(),
            )
            .map_err(RegistrationError::Publication)?
                != file.version_id
        {
            return Err(RegistrationError::CommitScope);
        }
        let mount_name =
            storage_contract::BranchName::parse(worktree.reservation().request.name.as_str())
                .map_err(RegistrationError::Allocation)?;
        if plan.source == plan.observed_head {
            if plan.destination != plan.source.branch_id
                || &plan.branch_name != source.branch_name()
            {
                return Err(RegistrationError::CommitScope);
            }
        } else {
            if plan.destination == plan.source.branch_id
                || plan.branch_name != mount_name
                || !allocations.branches.insert(plan.destination)
                || self.nodes.values().any(|node| {
                    node.plan.allocated.repository == plan.source.repository_id
                        && node.plan.branch_name.key() == plan.branch_name.key()
                })
                || self.commits.values().any(|node| {
                    node.node_plan.allocated.repository == plan.source.repository_id
                        && node.node_plan.branch_name.key() == plan.branch_name.key()
                })
                || self.historical.values().any(|record| {
                    record.plan.source.repository_id == plan.source.repository_id
                        && record.plan.branch_name.key() == plan.branch_name.key()
                })
            {
                return Err(RegistrationError::Identity);
            }
        }
        match &plan.file {
            HistoricalFile::Continue {
                branch: target,
                base,
            } => {
                let base = self
                    .source
                    .commits
                    .get(&base.to_string())
                    .ok_or(RegistrationError::CommitScope)?;
                if *target != branch
                    || plan.file_observed_head != node
                    || base.sequence > receipt.original.sequence
                    || base.entries.len() != 1
                    || base.entries[0].file_id != file.file_id
                    || base.entries[0].version_id != file.version_id
                {
                    return Err(RegistrationError::CommitScope);
                }
            }
            HistoricalFile::Fork {
                operation: child,
                name,
            } => {
                if name != &mount_name
                    || plan.file_observed_head == node
                    || *child == operation
                    || self.completions.contains_key(child)
                    || !self.planned_files.insert(*child)
                    || self
                        .source
                        .operations
                        .get(child)
                        .is_some_and(|record| record.sequence <= receipt.original.sequence)
                {
                    return Err(RegistrationError::Identity);
                }
            }
        }
        if !allocations.nodes.insert(plan.node)
            || plan.file_operation == operation
            || self.completions.contains_key(&plan.file_operation)
            || !self.planned_files.insert(plan.file_operation)
            || self
                .source
                .operations
                .get(&plan.file_operation)
                .is_some_and(|record| record.sequence <= receipt.original.sequence)
        {
            return Err(RegistrationError::Identity);
        }
        self.reserve_completion(operation, plan.ready, CompletionOwner::HistoricalReady)?;
        self.reserve_completion(
            operation,
            plan.node_publication,
            CompletionOwner::CommitNode(operation),
        )?;
        self.reserve_completion(operation, plan.completion, CompletionOwner::Commit)?;
        self.pending_edits
            .insert(plan.request.target.worktree_id, operation);
        self.pending_branches.insert(
            (plan.source.repository_id, plan.source.branch_id),
            operation,
        );
        self.pending_branches
            .insert((plan.source.repository_id, plan.destination), operation);
        self.historical.insert(
            operation,
            HistoricalRecord {
                receipt,
                plan,
                commitment,
            },
        );
        Ok(())
    }

    pub(super) fn prepare_historical(
        &mut self,
        ready_receipt: Receipt<'a>,
        ready: HistoricalReady,
        commitment: MaterialCommitment,
        allocations: &mut Allocations,
    ) -> Result<()> {
        let original = self
            .historical
            .get(&ready.reserve)
            .ok_or(RegistrationError::Operation(ready.reserve))?;
        let intent = original.plan.clone();
        require_completion(
            &original.receipt,
            &ready_receipt,
            intent.ready,
            original.commitment,
            ready.reservation,
        )?;
        if !intent.matches(&ready.plan) || self.commits.contains_key(&ready.reserve) {
            return Err(RegistrationError::CommitScope);
        }
        match &intent.file {
            HistoricalFile::Continue { branch, base } => {
                if ready.file_branch != *branch || ready.plan.file_base != *base {
                    return Err(RegistrationError::CommitScope);
                }
            }
            HistoricalFile::Fork { operation, name } => {
                let child = self
                    .source
                    .operations
                    .get(operation)
                    .ok_or(RegistrationError::Pending(ready.reserve))?;
                let actual = crate::persist::snapshot_worktree::historical::file_branch(
                    child,
                    &intent,
                    &original.receipt.principal,
                )
                .map_err(RegistrationError::Codec)?
                .ok_or(RegistrationError::CommitScope)?;
                let branch = self
                    .source
                    .branches
                    .get(&actual.branch_id.to_string())
                    .ok_or(RegistrationError::CommitScope)?;
                let base = self
                    .source
                    .commits
                    .get(&actual.commit_id.to_string())
                    .ok_or(RegistrationError::CommitScope)?;
                if child.sequence <= original.receipt.original.sequence
                    || child.sequence >= ready_receipt.original.sequence
                    || self
                        .source
                        .operation_principals
                        .get(operation)
                        .map(String::as_str)
                        != Some(original.receipt.principal.as_str())
                    || actual.branch_id != ready.file_branch
                    || actual.commit_id != ready.plan.file_base
                    || branch.file_id.as_deref()
                        != Some(intent.original.file_id.to_string().as_str())
                    || branch.history_tree_id != intent.file_history.to_string()
                    || branch.name != name.as_str()
                    || base.operation_id != *operation
                    || base.sequence != child.sequence
                    || base.kind != "file_branch_pending"
                {
                    return Err(RegistrationError::CommitScope);
                }
            }
        }
        let receipt = Receipt {
            original: original.receipt.original,
            principal: original.receipt.principal.clone(),
        };
        self.admit_commit(
            receipt,
            ready.plan,
            commitment,
            allocations,
            CommitRouting {
                directory: intent.directory(),
                file_branch: ready.file_branch,
            },
            intent.branch_name,
            ready_receipt.original.sequence,
            true,
        )
    }
}
