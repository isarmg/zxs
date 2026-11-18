//! Management only. The transport binds the current principal and registered
//! repository under the same writer lock as the reservation and decision.
use crate::{Engine, SnapshotWorktreeError};
use storage_contract::{OperationId, Principal, RepositoryId};
use storage_contract::snapshot_worktree::{WorktreeHandle, mount_change::{MountChange, MountChangeReceipt}};

impl Engine {
    pub fn directory_worktree_mount_owner(&self, target: WorktreeHandle) -> Result<RepositoryId, SnapshotWorktreeError> {
        self.snapshot_receipts()?.mount_owner(target).map_err(Into::into)
    }

    pub fn directory_worktree_mount_receipt(
        &self, repository: RepositoryId, operation: OperationId, actor: &Principal,
    ) -> Result<MountChangeReceipt, SnapshotWorktreeError> {
        self.snapshot_receipts()?.clean_mount_receipt(repository, operation, actor).map_err(Into::into)
    }

    pub fn change_directory_worktree_mount(
        &mut self, request: &MountChange, actor: &Principal,
    ) -> Result<MountChangeReceipt, SnapshotWorktreeError> {
        use crate::snapshot_worktree::RegistrationError;
        crate::snapshot_worktree::mount::change(self, request, actor).map_err(|source| {
            let occupied = self.state().operations.get(&request.operation_id).is_some_and(|r|
                r.kind == crate::persist::snapshot_worktree::MOUNT_RESERVE)
                && self.state().operation_principals.get(&request.operation_id).map(String::as_str) == Some(actor.as_str());
            if occupied && !matches!(source, RegistrationError::Pending(_) | RegistrationError::Digest(_)
                | RegistrationError::Operation(_)) {
                RegistrationError::DirectoryResponse { operation: request.operation_id, source: Box::new(source) }.into()
            } else { source.into() }
        })
    }
}
