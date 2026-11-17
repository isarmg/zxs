//! Only the admitted original and its independently completed receipt determine
//! a result. Neither current HEAD, a reused mount name nor disk bytes are queried.
use super::{CreationIntent, Publication, ReceiptIndex};
use storage_contract::snapshot_worktree::creation::WorktreeCreationRequest;
use crate::snapshot_worktree::{RegistrationError, Result};
use storage_contract::snapshot_worktree::WorktreeHandle;
use storage_contract::snapshot_worktree::outcome::{
    WorktreeCommitOutcome, WorktreeCommitReceipt, WorktreeCreationOutcome, WorktreeCreationReceipt,
};
use storage_contract::{OperationId, Principal, RepositoryId};

impl ReceiptIndex<'_> {
    pub(in crate::snapshot_worktree) fn creation_outcome(
        &self,
        repository: RepositoryId,
        operation: OperationId,
        principal: &Principal,
    ) -> Result<WorktreeCreationReceipt> {
        let record = self
            .worktrees
            .get(&operation)
            .ok_or(RegistrationError::Operation(operation))?;
        if record.receipt.principal != *principal
            || record.plan.host.namespace.repository_id != repository
        {
            return Err(RegistrationError::Digest(operation));
        }
        let result = match record.publication {
            Publication::Reserved => WorktreeCreationOutcome::Pending {},
            Publication::Complete(_) => WorktreeCreationOutcome::Complete {
                target: WorktreeHandle {
                    publication: record.plan.allocated.completion,
                    worktree_id: record.plan.allocated.worktree,
                },
            },
        };
        Ok(WorktreeCreationReceipt {
            repository_id: repository,
            request: match &record.intent {
                CreationIntent::Named => WorktreeCreationRequest::Named(record.plan.request.clone()),
                CreationIntent::Automatic(original) => {
                    WorktreeCreationRequest::Automatic(original.clone())
                }
            },
            result,
        })
    }

    pub(in crate::snapshot_worktree) fn commit_outcome(
        &self,
        repository: RepositoryId,
        operation: OperationId,
        principal: &Principal,
    ) -> Result<WorktreeCommitReceipt> {
        if let Some(original) = self.historical.get(&operation) {
            if original.receipt.principal != *principal
                || self.node(original.plan.source)?.host().namespace.repository_id != repository
            {
                return Err(RegistrationError::Digest(operation));
            }
            if !self.commits.contains_key(&operation) {
                return Ok(WorktreeCommitReceipt {
                    repository_id: repository, request: original.plan.request.clone(),
                    result: WorktreeCommitOutcome::Pending {},
                });
            }
        }
        let record = self
            .commits
            .get(&operation)
            .ok_or(RegistrationError::Operation(operation))?;
        if record.receipt.principal != *principal
            || record.node_plan.host.namespace.repository_id != repository
        {
            return Err(RegistrationError::Digest(operation));
        }
        let result = match record.completion {
            Publication::Reserved => WorktreeCommitOutcome::Pending {},
            Publication::Complete(_) => WorktreeCommitOutcome::Complete {
                snapshot: record.plan.handle_at(record.routing.directory.branch()),
            },
        };
        Ok(WorktreeCommitReceipt {
            repository_id: repository,
            request: record.plan.request.clone(),
            result,
        })
    }
}
