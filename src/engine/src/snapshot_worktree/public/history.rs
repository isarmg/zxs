//! A bounded metadata view of completed `.zxs` publications. Descriptions do
//! not grant a writer: creation still reopens its exact independently held node.
use crate::Engine;
use crate::snapshot_worktree::{RegistrationError, SnapshotWorktreeError};
use storage_contract::RepositoryId;
use storage_contract::snapshot_worktree::history::{
    DirectorySnapshotHistory, SnapshotHistoryCursor,
};
use storage_contract::snapshot_worktree::{NamespaceDirectoryId, NamespaceHostAddress};

impl Engine {
    /// Reads the actual registered root and original SYSTEM index, not ordinary
    /// directory Git or a filesystem scan. A new publication invalidates a page
    /// cursor; it never silently splices two observed receipt prefixes together.
    pub fn directory_snapshot_history(
        &self,
        repository: RepositoryId,
        cursor: Option<SnapshotHistoryCursor>,
    ) -> std::result::Result<DirectorySnapshotHistory, SnapshotWorktreeError> {
        let context = self.snapshot_root_context(repository)?;
        let root = self
            .namespace_host_root(repository, context.source.branch_id)
            .map_err(RegistrationError::Publication)?;
        let namespace = NamespaceHostAddress {
            repository_id: repository,
            branch_id: context.source.branch_id,
            directory: NamespaceDirectoryId::Repository(repository),
        };
        let index = self.snapshot_receipts()?;
        let (nodes, next_cursor) =
            index.history_at_root(namespace, context.host, root.binding(), cursor)?;
        Ok(DirectorySnapshotHistory {
            repository_id: repository,
            context,
            nodes,
            next_cursor,
        })
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/snapshot_worktree/history.rs"]
mod tests;
