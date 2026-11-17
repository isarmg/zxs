//! Current management authentication encloses the real SYSTEM decision and
//! held-root consumer. No public handle bypasses the independent receipt index.
pub(super) mod drafts;
pub(super) mod discovery;
pub(super) mod history;
pub(super) mod instances;
pub(super) mod root;
pub(super) mod outcome;
pub(super) mod transport;

use super::{EngineBackend, error_registry};
use crate::{ApiError, auth::Identity};
use storage_contract::snapshot_worktree::creation::AllocateWorktree;
use storage_contract::snapshot_worktree::{
    InitializeSnapshot, SnapshotHandle, SnapshotRootContext,
    WorktreeHandle, WorktreeStatus,
};

fn management(identity: &Identity) -> Result<(), ApiError> {
    if identity.smb_binding.is_some() { return Err(ApiError::forbidden()); }
    Ok(())
}

pub(in crate::engine_backend) fn error(source: storage_engine::SnapshotWorktreeError) -> ApiError {
    if let Some(operation) = source.published_operation() {
        let mut result = ApiError::new(
            http::StatusCode::SERVICE_UNAVAILABLE, storage_contract::ErrorCode::RecoveryRequired,
            format!(
                "operation {operation} is published; retain it and reconcile before more writes",
            ),
        );
        result.retryable = false;
        return result.with_source(source);
    }
    let code = error_registry::public_code(source.code());
    let mut result = ApiError::new(
        error_registry::status(code), code,
        "snapshot/worktree operation could not complete; retain any pending reservation",
    );
    result.retryable = source.retryable();
    result.with_source(source)
}

pub(super) fn context(
    backend: &EngineBackend, identity: &Identity, repository: storage_contract::RepositoryId,
) -> Result<SnapshotRootContext, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    management(&current)?;
    engine.snapshot_root_context(repository).map_err(error)
}

pub(super) fn initialize(
    backend: &EngineBackend, identity: &Identity, request: InitializeSnapshot,
) -> Result<SnapshotHandle, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    management(&current)?;
    let principal = current.principal.parse()
        .map_err(ApiError::invalid_authenticated_principal)?;
    engine.initialize_directory_snapshot(&request, &principal).map_err(error)
}

pub(super) fn create(
    backend: &EngineBackend, identity: &Identity, request: AllocateWorktree,
) -> Result<WorktreeHandle, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    management(&current)?;
    let principal = current.principal.parse()
        .map_err(ApiError::invalid_authenticated_principal)?;
    engine.allocate_directory_worktree(&request, &principal).map_err(error)
}

pub(super) fn status(
    backend: &EngineBackend, identity: &Identity, target: WorktreeHandle,
) -> Result<WorktreeStatus, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    management(&current)?;
    engine.directory_worktree_status(target).map_err(error)
}
