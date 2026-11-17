//! Current identity and root registration enclose the one authoritative history read.
use crate::ApiError;
use crate::auth::Identity;
use crate::engine_backend::EngineBackend;
use crate::engine_backend::snapshot_worktree::{error, management};
use storage_contract::RepositoryId;
use storage_contract::snapshot_worktree::history::{
    DirectorySnapshotHistory, SnapshotHistoryCursor,
};

pub(in crate::engine_backend) fn read(
    backend: &EngineBackend,
    identity: &Identity,
    repository: RepositoryId,
    cursor: Option<SnapshotHistoryCursor>,
) -> Result<DirectorySnapshotHistory, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    management(&current)?;
    engine
        .directory_snapshot_history(repository, cursor)
        .map_err(error)
}
