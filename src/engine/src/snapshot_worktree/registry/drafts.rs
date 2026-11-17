//! Directory selection borrows a complete published head, never a reserved node.
use crate::snapshot_worktree::registry::{PublishedNode, ReceiptIndex};
use crate::snapshot_worktree::{RegistrationError, Result};
use storage_contract::snapshot_worktree::SnapshotHandle;

impl ReceiptIndex<'_> {
    /// Pending and completed nested registrations both occupy their original
    /// namespace entry. Never discover a host from a local management filename.
    pub(crate) fn contains_nested_draft_host(
        &self, repository: storage_contract::RepositoryId, branch: storage_contract::BranchId,
        parents: &[(storage_contract::EntryId, name_policy::PortablePath)],
    ) -> bool {
        use storage_contract::snapshot_worktree::NamespaceDirectoryId;
        !parents.is_empty() && self.nodes.values().any(|record| {
            let ns = &record.plan.host.namespace;
            ns.repository_id == repository && ns.branch_id == branch
                && matches!(ns.directory, NamespaceDirectoryId::Entry(id)
                    if parents.iter().any(|(parent, _)| *parent == id))
        })
    }

    pub(in crate::snapshot_worktree) fn draft_head(
        &self,
        handle: SnapshotHandle,
    ) -> Result<PublishedNode<'_>> {
        let published = self.node(handle)?;
        let branch = (handle.repository_id, handle.branch_id);
        if let Some(operation) = self.pending_branches.get(&branch) {
            return Err(RegistrationError::Pending(*operation));
        }
        if self.branch_heads.get(&branch) != Some(&handle) {
            return Err(RegistrationError::Source);
        }
        Ok(published)
    }
}
