//! Read original outcomes without retrying a command or adopting local state.
use crate::{Engine, SnapshotWorktreeError};
use storage_contract::snapshot_worktree::outcome::{
    WorktreeCommitReceipt, WorktreeCreationReceipt,
};
use storage_contract::{OperationId, Principal, RepositoryId};

impl Engine {
    pub fn directory_worktree_creation_receipt(
        &self,
        repository: RepositoryId,
        operation: OperationId,
        principal: &Principal,
    ) -> Result<WorktreeCreationReceipt, SnapshotWorktreeError> {
        self.snapshot_receipts()?
            .creation_outcome(repository, operation, principal)
            .map_err(Into::into)
    }

    pub fn directory_worktree_commit_receipt(
        &self,
        repository: RepositoryId,
        operation: OperationId,
        principal: &Principal,
    ) -> Result<WorktreeCommitReceipt, SnapshotWorktreeError> {
        self.snapshot_receipts()?
            .commit_outcome(repository, operation, principal)
            .map_err(Into::into)
    }
}
