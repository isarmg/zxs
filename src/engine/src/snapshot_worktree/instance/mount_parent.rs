//! A sparse instance is a virtual sibling, not a physical directory. Keep the
//! original snapshot's host distinct from the ordinary parent issuing its name.
use crate::persist::snapshot_worktree::HostEvidence;
use crate::snapshot_worktree::{RegistrationError, Result};
use storage_contract::snapshot_worktree::{NamespaceDirectoryId, NamespaceHostAddress};
use storage_contract::working_tree::DirectoryId;

#[derive(Clone, Copy)]
pub(in crate::snapshot_worktree) struct MountParent {
    pub(in crate::snapshot_worktree) namespace: NamespaceHostAddress,
    pub(in crate::snapshot_worktree) directory: DirectoryId,
}
impl MountParent {
    /// Called only with index-admitted original host evidence. A handle to an
    /// arbitrary ordinary nested directory, even on this disk, is not a mount.
    pub(in crate::snapshot_worktree) fn original(host: &HostEvidence) -> Result<Self> {
        let repository = host.namespace.repository_id;
        let directory = host.common_data.directory_id;
        match (host.namespace.directory, host.directories.as_slice()) {
            (NamespaceDirectoryId::Repository(id), []) if id == repository => {}
            (NamespaceDirectoryId::Entry(id), [child]) if id == child.entry
                && child.directory.parent_directory_id == directory && child.directory.directory_id != directory
                && child.directory.entity != host.common_data.entity => {}
            _ => return Err(RegistrationError::RootOnly),
        }
        Ok(Self { namespace:NamespaceHostAddress {
            directory:NamespaceDirectoryId::Repository(repository), ..host.namespace
        }, directory })
    }
}
