//! Descriptive transport requests enter the existing sealed edit and MOVE
//! publication. A returned read retains the admitted managed-disk descriptor.
use crate::directory_worktree::io::EntryContent;
use crate::directory_worktree::session::ReadMaterial;
use crate::snapshot_worktree::{RegistrationError, SnapshotWorktreeError};
use crate::{Engine, ReadFile};
use name_policy::LookupPath;
use storage_contract::snapshot_worktree::{
    CommitWorktree, EditWorktree, SnapshotHandle, WorktreeHandle, WorktreeRevision,
};
use storage_contract::{ContentHash, Principal, RepositoryId};

type Result<T> = std::result::Result<T, SnapshotWorktreeError>;

pub struct WorktreeContent {
    pub file: ReadFile,
    pub size: u64,
    pub content_hash: ContentHash,
    pub revision: WorktreeRevision,
}

impl Engine {
    /// Permanent owner lookup precedes mutation or byte disclosure. A caller
    /// must authorize this repository with its current identity under its lock.
    pub fn directory_worktree_owner(&self, target: WorktreeHandle) -> Result<RepositoryId> {
        let index = self.snapshot_receipts()?;
        Ok(index.worktree(target)?.source().host().namespace.repository_id)
    }

    pub fn open_directory_worktree_content(
        &self,
        target: WorktreeHandle,
        path: &LookupPath,
    ) -> Result<WorktreeContent> {
        let index = self.snapshot_receipts()?;
        let registered = index.worktree(target)?;
        let original = index.reopen_node(self, registered.source().handle())?;
        let session = registered.reopen(&original)?;
        let published = registered.published_node()
            .map(|handle| index.reopen_node(self, handle)).transpose()?;
        let material = match &published {
            Some(node) => ReadMaterial::Published(node),
            None => ReadMaterial::Original,
        };
        let view = session.read(material).map_err(RegistrationError::ReadView)?;
        let EntryContent::File { material } = view.content(path)
            .map_err(RegistrationError::ReadView)? else {
                return Err(RegistrationError::ReadView(
                    crate::directory_worktree::io::ViewError::IsDirectory,
                ).into());
            };
        let file = view.open_file(path).map_err(RegistrationError::ReadView)?;
        Ok(WorktreeContent {
            file,
            size: material.size,
            content_hash: material.content_hash,
            revision: session.worktree().revision(),
        })
    }

    /// A complete body for the same original occurrence before directory commit.
    /// This is not a shared write handle and does not publish any history node.
    pub fn edit_directory_worktree(
        &mut self,
        request: &EditWorktree,
        principal: &Principal,
        mut source: ReadFile,
    ) -> Result<WorktreeRevision> {
        crate::snapshot_worktree::edit_worktree(self, request, &mut source, principal)
            .map_err(Into::into)
    }

    pub fn commit_directory_worktree(
        &mut self,
        request: &CommitWorktree,
        principal: &Principal,
    ) -> Result<SnapshotHandle> {
        crate::snapshot_worktree::commit_worktree(self, request, principal).map_err(Into::into)
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/snapshot_worktree/complete_resaves.rs"]
mod tests;
