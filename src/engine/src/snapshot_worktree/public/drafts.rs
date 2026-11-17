//! The public command carries descriptions only. Existing handling, the full
//! selected-set CAS and the one physical decision remain inside the Engine.
use crate::snapshot_worktree::drafts::DirectoryDraftIntent;
use crate::snapshot_worktree::public::SnapshotWorktreeError;
use crate::snapshot_worktree::RegistrationError;
use crate::Engine;
use storage_contract::snapshot_worktree::draft_commit::{
    CommitDirectoryDrafts, DirectoryDraftLookup, DirectoryDraftReceipt,
};
use storage_contract::snapshot_worktree::SnapshotHandle;
use storage_contract::{Principal, RepositoryId};

type Result<T> = std::result::Result<T, SnapshotWorktreeError>;

impl Engine {
    /// Permanent source ownership only; this does not require current HEAD or
    /// open draft bytes. Servers use it for resource authorization before retry.
    pub fn directory_snapshot_owner(&self, source: SnapshotHandle) -> Result<RepositoryId> {
        let index = self.snapshot_receipts()?;
        Ok(index.node(source)?.host().namespace.repository_id)
    }

    pub fn directory_draft_receipt(
        &self, lookup: DirectoryDraftLookup, principal: &Principal,
    ) -> Result<DirectoryDraftReceipt> {
        self.snapshot_receipts()?.public_working_receipt(lookup, principal).map_err(Into::into)
    }

    pub fn commit_directory_drafts(
        &mut self, request: &CommitDirectoryDrafts, principal: &Principal,
    ) -> Result<DirectoryDraftReceipt> {
        let selected: Vec<_> = request.selected.iter().map(|item| item.selection()).collect();
        let result = self.commit_directory_working_set(DirectoryDraftIntent {
            source: request.source, selected: &selected, operation: request.operation_id,
            principal, message: &request.message,
        }).map_err(RegistrationError::Publication)?;
        self.directory_draft_receipt(DirectoryDraftLookup {
            source: request.source, operation_id: result.operation_id,
        }, principal).map_err(|source| RegistrationError::DirectoryResponse {
            operation: result.operation_id, source: Box::new(source.source),
        }.into())
    }
}
