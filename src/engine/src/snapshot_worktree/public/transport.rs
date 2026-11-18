//! Descriptive transport requests enter the existing sealed edit and MOVE
//! publication. A returned read retains the admitted managed-disk descriptor.
use crate::snapshot_worktree::{RegistrationError, SnapshotWorktreeError};
use crate::snapshot_worktree::public::namespace::with_view;
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

    /// Management commit authorization only. A registered pending operation
    /// may reach its own retry; reads and held SMB handles still require Ready.
    pub fn directory_worktree_commit_owner(
        &self, request: &CommitWorktree, principal: &Principal,
    ) -> Result<RepositoryId> {
        self.snapshot_receipts()?.commit_owner(request, principal).map_err(Into::into)
    }

    pub fn open_directory_worktree_content(
        &self,
        target: WorktreeHandle,
        path: &LookupPath,
    ) -> Result<WorktreeContent> {
        with_view(self, target, |view| {
            let opened = view.open_file(path).map_err(RegistrationError::ReadView)?;
            Ok(WorktreeContent {
                file: opened.reader,
                size: opened.material.size,
                content_hash: opened.material.content_hash,
                revision: view.stamp().revision,
            })
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

#[cfg(test)]
#[path = "../../../../../test/engine/unit/snapshot_worktree/content_overlay.rs"]
mod content_overlay;
