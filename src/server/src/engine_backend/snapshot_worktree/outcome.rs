//! One current management lock covers ordinary namespace and original author.
use super::{error, management};
use crate::engine_backend::{EngineBackend, operations, repository};
use crate::{ApiError, Identity};
use storage_contract::snapshot_worktree::outcome::{
    WorktreeCommitReceipt, WorktreeCreationReceipt,
};
use storage_contract::{OperationId, RepositoryId};

pub(in crate::engine_backend) fn creation(
    backend: &EngineBackend,
    identity: &Identity,
    owner: RepositoryId,
    operation: OperationId,
) -> Result<WorktreeCreationReceipt, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    management(&current)?;
    operations::authorized_record(&engine, &current, operation)?;
    repository(&engine, &owner.to_string())?;
    let principal = current
        .principal
        .parse()
        .map_err(ApiError::invalid_authenticated_principal)?;
    engine
        .directory_worktree_creation_receipt(owner, operation, &principal)
        .map_err(error)
}

pub(in crate::engine_backend) fn commit(
    backend: &EngineBackend,
    identity: &Identity,
    owner: RepositoryId,
    operation: OperationId,
) -> Result<WorktreeCommitReceipt, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    management(&current)?;
    operations::authorized_record(&engine, &current, operation)?;
    repository(&engine, &owner.to_string())?;
    let principal = current
        .principal
        .parse()
        .map_err(ApiError::invalid_authenticated_principal)?;
    engine
        .directory_worktree_commit_receipt(owner, operation, &principal)
        .map_err(error)
}
