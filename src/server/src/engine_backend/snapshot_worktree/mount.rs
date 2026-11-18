//! No hidden enumeration. The full retained target permits an ownership check,
//! not an ordinary read, I/O, receipt lookup for another actor or new identity.
use crate::engine_backend::{EngineBackend, current_protected_identity, operations, repository};
use crate::engine_backend::admission::AdmissionEngine;
use crate::engine_backend::snapshot_worktree::{error, management};
use crate::{ApiError, Identity};
use storage_contract::{OperationId, Principal, RepositoryId};
use storage_contract::snapshot_worktree::mount_change::{MountAction, MountChange, MountChangeReceipt, MountOutcome};

fn authorize(
    engine: &AdmissionEngine, identity: &Identity, owner: RepositoryId, action: &MountAction,
) -> Result<Principal, ApiError> {
    // Both configured authentication sources independently revalidate against
    // their sole configured administrator and generation. A protocol mapping
    // has no management authority, even if its principal has the same spelling.
    let current = current_protected_identity(engine, identity)?;
    management(&current)?;
    repository(engine, &owner.to_string())?;
    let actor = match action {
        MountAction::Hide { .. } => current,
        // The existing management auth owner is single-administrator. Compare
        // the current authoritative mapping; do not hardcode the name "admin"
        // or treat a past delete receipt as an administrator credential.
        MountAction::Restore { .. } => engine.identity(&current)?,
    };
    actor.principal.parse().map_err(ApiError::invalid_authenticated_principal)
}

pub(in crate::engine_backend) fn change(
    backend: &EngineBackend, identity: &Identity, request: MountChange,
) -> Result<MountChangeReceipt, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    management(&current)?;
    let owner = engine.directory_worktree_mount_owner(request.target).map_err(error)?;
    let actor = authorize(&engine, &current, owner, &request.action)?;
    let reserved = engine.reserve_directory_worktree_mount(&request, &actor).map_err(error)?;
    if !matches!(reserved.outcome, MountOutcome::Pending {}) { return Ok(reserved); }
    let revalidated = authorize(&engine, &current, owner, &request.action).map_err(after_reservation)?;
    if revalidated != actor { return Err(after_reservation(ApiError::forbidden())); }
    engine.change_directory_worktree_mount(&request, &actor).map_err(error).map_err(after_reservation)
}

/// A generic response cannot prove that an already committed original claim
/// failed. Never disclose the reserved name, contents or receipt to a session
/// that may have lost access during preparation. Keep the private cause.
fn after_reservation(mut source: ApiError) -> ApiError {
    source.status = http::StatusCode::SERVICE_UNAVAILABLE;
    source.code = storage_contract::ErrorCode::RecoveryRequired;
    source.message = "outcome unavailable; keep the same original request and operation".into();
    source.retryable = false;
    source
}

pub(in crate::engine_backend) fn receipt(
    backend: &EngineBackend, identity: &Identity, owner: RepositoryId, op: OperationId,
) -> Result<MountChangeReceipt, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    management(&current)?;
    repository(&engine, &owner.to_string())?;
    operations::authorized_record(&engine, &current, op)?;
    let actor = current.principal.parse().map_err(ApiError::invalid_authenticated_principal)?;
    let receipt = engine.directory_worktree_mount_receipt(owner, op, &actor).map_err(error)?;
    if authorize(&engine, &current, owner, &receipt.request.action)? != actor { return Err(ApiError::forbidden()); }
    Ok(receipt)
}
