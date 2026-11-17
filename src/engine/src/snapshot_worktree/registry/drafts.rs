//! Directory selection borrows a complete published head, never a reserved node.
use crate::snapshot_worktree::registry::{PublishedNode, ReceiptIndex};
use crate::snapshot_worktree::{RegistrationError, Result};
use storage_contract::snapshot_worktree::SnapshotHandle;

impl ReceiptIndex<'_> {
    pub(in crate::snapshot_worktree) fn draft_head(
        &self,
        handle: SnapshotHandle,
    ) -> Result<PublishedNode<'_>> {
        let published = self.node(handle)?;
        let branch = (handle.repository_id, handle.branch_id);
        if let Some(operation) = self.pending_branches.get(&branch) {
            return Err(RegistrationError::Pending(*operation));
        }
        if self.branch_heads.get(&branch) != Some(&handle.node_id) {
            return Err(RegistrationError::Source);
        }
        Ok(published)
    }
}
