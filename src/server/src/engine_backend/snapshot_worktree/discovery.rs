//! Context and discovery share current credential admission and engine ownership.
use crate::auth::Identity;
use crate::engine_backend::EngineBackend;
use crate::engine_backend::snapshot_worktree::{error, management};
use crate::ApiError;
use storage_contract::RepositoryId;
use storage_contract::snapshot_worktree::{NamespaceDirectoryId, NamespaceHostAddress};
use storage_contract::snapshot_worktree::discovery::{
    DirectorySnapshotHead, SnapshotBranchSelection, SnapshotHeadRequest,
};

pub(in crate::engine_backend) fn head(
    backend: &EngineBackend,
    identity: &Identity,
    repository: RepositoryId,
) -> Result<DirectorySnapshotHead, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    management(&current)?;
    let context = engine.snapshot_root_context(repository).map_err(error)?;
    let head = engine.discover_directory_snapshot(SnapshotHeadRequest {
        namespace: NamespaceHostAddress {
            repository_id: repository,
            branch_id: context.source.branch_id,
            directory: NamespaceDirectoryId::Repository(repository),
        },
        host: context.host,
        branch: SnapshotBranchSelection::Initial,
    }).map_err(error)?;
    Ok(DirectorySnapshotHead { repository_id: repository, context, head })
}
