//! Metadata discovery binds the current ordinary namespace owner before
//! reading its installed original index. It neither opens nor creates `.zxs`.
use crate::snapshot_worktree::RegistrationError;
use crate::snapshot_worktree::public::SnapshotWorktreeError;
use crate::Engine;
use storage_contract::snapshot_worktree::NamespaceDirectoryId;
use storage_contract::snapshot_worktree::discovery::{SnapshotHeadDiscovery, SnapshotHeadRequest};

type Result<T> = std::result::Result<T, SnapshotWorktreeError>;

impl Engine {
    /// Discover the initial or explicitly designated formal branch head in a
    /// registered root. This is a read, not write admission or a selected-branch
    /// preference. Callers still authorize this exact namespace before use.
    pub fn discover_directory_snapshot(
        &self, request: SnapshotHeadRequest,
    ) -> Result<SnapshotHeadDiscovery> {
        if request.namespace.directory
            != NamespaceDirectoryId::Repository(request.namespace.repository_id)
        {
            return Err(RegistrationError::RootOnly.into());
        }
        let root = self.namespace_host_root(
            request.namespace.repository_id, request.namespace.branch_id,
        ).map_err(RegistrationError::Publication)?;
        let binding = root.binding();
        if request.host.root_id != binding.root_id
            || request.host.directory_id != binding.roots.common_data.directory_id
            || request.host.expected_root_generation != binding.generation
        {
            return Err(RegistrationError::PublicationChanged.into());
        }
        self.snapshot_receipts()?.discover_root(request, binding).map_err(Into::into)
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/snapshot_worktree/head_discovery.rs"]
mod tests;
