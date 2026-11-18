//! Public commands retain the existing independent reservation/completion
//! publishers. No transport input constructs a host, session or local anchor.
mod drafts;
mod discovery;
mod history;
mod transport;
mod namespace;
mod outcome;
mod mount;
pub use transport::WorktreeContent;
pub use namespace::{
    WorktreeCursor, WorktreeEntry, WorktreeEntryContent, WorktreePage, WorktreePageStamp,
};

use super::RegistrationError;
use crate::Engine;
use crate::snapshot_worktree::registry::ReceiptIndex;
use storage_contract::{ErrorCode, Generation, Principal, RepositoryId};
use storage_contract::snapshot_worktree::{
    CreateWorktree, InitializeSnapshot, SnapshotHandle, SnapshotHost, SnapshotNamespace,
    SnapshotRootContext, WorktreeHandle, WorktreeStatus,
};

/// An opaque error preserves the concrete storage cause without exporting the
/// private owner/preparation types as constructors of public capabilities.
#[derive(Debug, error_definition::Error)]
#[error("snapshot/worktree operation could not complete")]
pub struct SnapshotWorktreeError {
    #[source]
    source: RegistrationError,
}

impl From<RegistrationError> for SnapshotWorktreeError {
    fn from(source: RegistrationError) -> Self { Self { source } }
}

impl SnapshotWorktreeError {
    pub fn code(&self) -> ErrorCode {
        Self::code_for_registration(&self.source)
    }

    pub(crate) fn code_for_registration(source: &RegistrationError) -> ErrorCode {
        match source {
            RegistrationError::Publication(source) => source.code(),
            RegistrationError::RetainedName(source) => source.code(),
            RegistrationError::OriginalReceipt(source) => source.code(),
            RegistrationError::Allocation(source) => source.code(),
            RegistrationError::DirectoryRecord(source) => source.code(),
            RegistrationError::DirectoryResult(source) => source.code(),
            RegistrationError::InitialOnly | RegistrationError::RootOnly
            | RegistrationError::EditScope | RegistrationError::CommitScope => {
                ErrorCode::UnsupportedOperation
            }
            RegistrationError::DirectoryResponse { .. }
            | RegistrationError::Pending(_) | RegistrationError::OriginalReceiptUnavailable(_)
            | RegistrationError::CommitWorkspaceRelease { .. } => {
                ErrorCode::RecoveryRequired
            }
            RegistrationError::Operation(_) | RegistrationError::Digest(_) => {
                ErrorCode::IdempotencyKeyReused
            }
            RegistrationError::Host | RegistrationError::Mount => ErrorCode::NameCollision,
            RegistrationError::WorktreeName(source) => match source {
                crate::directory_worktree::names::NameError::Exhausted => {
                    ErrorCode::ResourceLimitExceeded
                }
                crate::directory_worktree::names::NameError::TooLong => ErrorCode::NameTooLong,
                crate::directory_worktree::names::NameError::Invalid(source)
                | crate::directory_worktree::names::NameError::MountedPath(source) => {
                    ErrorCode::from(source)
                }
            },
            RegistrationError::PublicationChanged => ErrorCode::PreparationStale,
            RegistrationError::EditBody
            | RegistrationError::EditVerification(platform_io::payload::Error::Length { .. }
                | platform_io::payload::Error::Digest) => ErrorCode::ChecksumMismatch,
            RegistrationError::ReadView(crate::directory_worktree::io::ViewError::Missing) => {
                ErrorCode::NotFound
            }
            RegistrationError::ReadView(crate::directory_worktree::io::ViewError::IsDirectory) => {
                ErrorCode::IsDirectory
            }
            RegistrationError::ReadView(crate::directory_worktree::io::ViewError::NotDirectory) => {
                ErrorCode::NotDirectory
            }
            RegistrationError::ReadView(
                crate::directory_worktree::io::ViewError::WorkingSourceRequired,
            ) => {
                ErrorCode::UnsupportedOperation
            }
            RegistrationError::HistoryCursor
            | RegistrationError::ReadView(crate::directory_worktree::io::ViewError::Cursor) => {
                ErrorCode::ListingChanged
            }
            RegistrationError::Ledger(crate::directory_worktree::ledger::LedgerError::FrameLimit)
            | RegistrationError::ReadView(crate::directory_worktree::io::ViewError::Budget) => {
                ErrorCode::ResourceLimitExceeded
            }
            _ => ErrorCode::MetadataUnavailable,
        }
    }

    /// A lower publication may have committed before local projection failed.
    /// Adapters must not turn this into permission to start another operation.
    pub fn published_operation(&self) -> Option<storage_contract::OperationId> {
        match &self.source {
            RegistrationError::DirectoryResponse { operation, .. } => Some(*operation),
            RegistrationError::Publication(source) => {
                source.published_result().map(|result| result.operation_id)
            }
            _ => None,
        }
    }

    pub fn retryable(&self) -> bool {
        match &self.source {
            RegistrationError::Publication(source) => source.is_retryable(),
            _ => false,
        }
    }
}

type Result<T> = std::result::Result<T, SnapshotWorktreeError>;

impl Engine {
    /// Resolve the real published file-manager root, not the same-looking
    /// directory or a Config entry. The subsequent command revalidates it.
    pub fn snapshot_root_context(&self, repository: RepositoryId) -> Result<SnapshotRootContext> {
        let repository = self.state().repositories.get(&repository.to_string())
            .ok_or(RegistrationError::Source)?;
        let branch_id = repository.main_branch_id.parse().map_err(RegistrationError::Allocation)?;
        let branch = self.state().branches.get(&repository.main_branch_id)
            .ok_or(RegistrationError::Source)?;
        let root = self.namespace_host_root(
            repository.repository_id.parse().map_err(RegistrationError::Allocation)?, branch_id,
        ).map_err(RegistrationError::Publication)?;
        root.common().map_err(RegistrationError::Publication)?;
        let binding = root.binding();
        Ok(SnapshotRootContext {
            host: SnapshotHost {
                root_id: binding.root_id,
                directory_id: binding.roots.common_data.directory_id,
                expected_root_generation: binding.generation,
            },
            source: SnapshotNamespace {
                branch_id, root: None,
                expected_branch_generation: Generation::new(branch.generation),
            },
        })
    }

    pub fn initialize_directory_snapshot(
        &mut self, request: &InitializeSnapshot, principal: &Principal,
    ) -> Result<SnapshotHandle> {
        super::initialize_root(self, request, principal).map_err(Into::into)
    }

    pub fn create_directory_worktree(
        &mut self, request: &CreateWorktree, principal: &Principal,
    ) -> Result<WorktreeHandle> {
        super::create_worktree(self, request, principal).map_err(Into::into)
    }

    pub fn allocate_directory_worktree(
        &mut self,
        request: &storage_contract::snapshot_worktree::creation::AllocateWorktree,
        principal: &Principal,
    ) -> Result<WorktreeHandle> {
        super::allocate_worktree(self, request, principal).map_err(Into::into)
    }

    pub fn directory_worktree_status(&self, target: WorktreeHandle) -> Result<WorktreeStatus> {
        self.snapshot_receipts()?.status(self, target)
    }
}

impl ReceiptIndex<'_> {
    fn status(&self, engine: &Engine, target: WorktreeHandle) -> Result<WorktreeStatus> {
        let registered = self.worktree(target)?;
        let crate::snapshot_worktree::registry::Presence::Visible(visible) = registered.mount.presence
        else { return Err(RegistrationError::Source.into()); };
        let original = registered.source().handle();
        let held = self.reopen_node(engine, original)?;
        let session = registered.reopen(&held)?;
        let worktree = session.worktree();
        Ok(WorktreeStatus {
            visible,
            target,
            original,
            parent_directory_id: worktree.mount().parent,
            name: worktree.mount().name.clone(),
            revision: worktree.revision(),
        })
    }
}
