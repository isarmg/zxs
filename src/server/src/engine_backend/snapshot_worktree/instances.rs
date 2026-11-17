//! Reuse the mount authority with a pre-reopen response budget under one current lock.
use crate::ApiError;
use crate::auth::Identity;
use crate::engine_backend::EngineBackend;
use crate::engine_backend::snapshot_worktree::{error, management};
use storage_contract::RepositoryId;
use storage_contract::snapshot_worktree::instances::{
    DIRECTORY_WORKTREE_LIST_LIMIT, DirectoryWorktrees,
};

pub(in crate::engine_backend) fn read(
    backend: &EngineBackend,
    identity: &Identity,
    repository: RepositoryId,
) -> Result<DirectoryWorktrees, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    management(&current)?;
    let context = engine.snapshot_root_context(repository).map_err(error)?;
    let instances = engine
        .directory_worktree_mounts_bounded(repository, DIRECTORY_WORKTREE_LIST_LIMIT)
        .map_err(error)?;
    Ok(DirectoryWorktrees {
        repository_id: repository,
        context,
        instances,
    })
}
