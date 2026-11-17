//! Live management authority encloses exact worktree ownership and the real
//! edit/commit decision. A path never selects another instance or container.
use crate::auth::Identity;
use crate::engine_backend::{EngineBackend, file_draft, repository};
use crate::engine_backend::snapshot_worktree::{error, management};
use crate::{ApiError, Content, UploadSource};
use name_policy::LookupPath;
use storage_contract::snapshot_worktree::{
    CommitWorktree, EditWorktree, SnapshotHandle, WorktreeHandle, WorktreeRevision,
};

fn authorize(
    engine: &storage_engine::Engine,
    current: &Identity,
    target: WorktreeHandle,
) -> Result<(), ApiError> {
    management(current)?;
    let owner = engine.directory_worktree_owner(target).map_err(error)?;
    repository(engine, &owner.to_string())?;
    Ok(())
}

pub(in crate::engine_backend) fn authorize_edit(
    backend: &EngineBackend,
    identity: &Identity,
    request: &EditWorktree,
) -> Result<(), ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    authorize(&engine, &current, request.target)?;
    if request.size > engine.config().max_file_bytes {
        return Err(ApiError::limit("worktree body exceeds the configured file limit"));
    }
    Ok(())
}

pub(in crate::engine_backend) fn edit(
    backend: &EngineBackend,
    identity: &Identity,
    request: EditWorktree,
    source: &UploadSource,
) -> Result<WorktreeRevision, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    authorize(&engine, &current, request.target)?;
    if source.hash != request.content_hash || source.size != request.size {
        return Err(ApiError::invalid("worktree upload differs from its edit command"));
    }
    let principal = current.principal.parse().map_err(ApiError::invalid_authenticated_principal)?;
    let file = source.file.try_clone().map_err(ApiError::io)?;
    engine.edit_directory_worktree(&request, &principal, file.into()).map_err(error)
}

pub(in crate::engine_backend) fn commit(
    backend: &EngineBackend,
    identity: &Identity,
    request: CommitWorktree,
) -> Result<SnapshotHandle, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    management(&current)?;
    let principal = current.principal.parse().map_err(ApiError::invalid_authenticated_principal)?;
    let owner = engine.directory_worktree_commit_owner(&request, &principal).map_err(error)?;
    repository(&engine, &owner.to_string())?;
    engine.commit_directory_worktree(&request, &principal).map_err(error)
}

pub(in crate::engine_backend) fn content(
    backend: &EngineBackend,
    identity: &Identity,
    target: WorktreeHandle,
    path: LookupPath,
    deadline: std::time::Instant,
) -> Result<Content, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    authorize(&engine, &current, target)?;
    let content = engine.open_directory_worktree_content(target, &path).map_err(error)?;
    drop(engine);
    file_draft::verified_content(content.size, content.content_hash, deadline, || Ok(content.file))
}
