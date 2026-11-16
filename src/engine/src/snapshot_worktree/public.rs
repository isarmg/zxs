//! Public commands retain the existing independent reservation/completion
//! publishers. No transport input constructs a host, session or local anchor.
use super::{RegistrationError, registry::ReceiptIndex};
use crate::Engine;
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
        match &self.source {
            RegistrationError::Publication(source) => source.code(),
            RegistrationError::Allocation(source) => source.code(),
            RegistrationError::InitialOnly | RegistrationError::RootOnly
            | RegistrationError::EditScope | RegistrationError::CommitScope => {
                ErrorCode::UnsupportedOperation
            }
            RegistrationError::Pending(_) => ErrorCode::RecoveryRequired,
            RegistrationError::Operation(_) | RegistrationError::Digest(_) => {
                ErrorCode::IdempotencyKeyReused
            }
            RegistrationError::Host | RegistrationError::Mount => ErrorCode::NameCollision,
            RegistrationError::PublicationChanged => ErrorCode::PreparationStale,
            RegistrationError::EditBody => ErrorCode::ChecksumMismatch,
            _ => ErrorCode::MetadataUnavailable,
        }
    }

    /// A lower publication may have committed before local projection failed.
    /// Adapters must not turn this into permission to start another operation.
    pub fn published_operation(&self) -> Option<storage_contract::OperationId> {
        match &self.source {
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

    pub fn directory_worktree_status(&self, target: WorktreeHandle) -> Result<WorktreeStatus> {
        let index = ReceiptIndex::capture(self.state())?;
        let original = index.worktree(target)?.source().handle();
        super::with_worktree(self, target, |session| {
            let worktree = session.worktree();
            Ok(WorktreeStatus {
                target, original,
                parent_directory_id: worktree.mount().parent,
                name: worktree.mount().name.clone(),
                revision: worktree.revision(),
            })
        }).map_err(Into::into)
    }
}
