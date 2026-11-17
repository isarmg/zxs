//! Preserve the existing creation and exact-operation kernel under current auth.
use crate::ApiError;
use crate::auth::Identity;
use crate::engine_backend::snapshot_worktree::management;
use crate::engine_backend::{EngineBackend, error, file_manager};
use name_policy::SlotId;
use storage_contract::snapshot_worktree::root::{RegisterSnapshotRoot, RegisteredSnapshotRoot};

pub(in crate::engine_backend) fn register(
    backend: &EngineBackend,
    identity: &Identity,
    slot: SlotId,
    request: RegisterSnapshotRoot,
) -> Result<RegisteredSnapshotRoot, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    management(&current)?;
    file_manager::require_primary_disk(&file_manager::disk(&engine, slot.as_str())?)?;
    let repository = engine
        .create_disk_root(slot.as_str(), &current.principal, request.operation_id)
        .map_err(error)?;
    Ok(RegisteredSnapshotRoot {
        slot_id: slot,
        repository_id: repository
            .repository_id
            .parse()
            .map_err(ApiError::contract_input)?,
    })
}
