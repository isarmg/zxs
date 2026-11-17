//! One current management lock covers all selected resource checks and the
//! native decision. Authorization does not recapture working revision or HEAD.
use crate::auth::Identity;
use crate::engine_backend::{EngineBackend, file_draft, operations, repository};
use crate::engine_backend::snapshot_worktree::{error, management};
use crate::ApiError;
use storage_contract::snapshot_worktree::draft_commit::{
    CommitDirectoryDrafts, DirectoryDraftExpectation, DirectoryDraftLookup, DirectoryDraftReceipt,
};
use storage_contract::snapshot_worktree::SnapshotHandle;
use storage_contract::RepositoryId;

fn authorize_source(
    engine: &storage_engine::Engine, current: &Identity, source: SnapshotHandle,
) -> Result<RepositoryId, ApiError> {
    management(current)?;
    let owner = engine.directory_snapshot_owner(source).map_err(error)?;
    repository(engine, &owner.to_string())?;
    Ok(owner)
}

pub(in crate::engine_backend) fn commit(
    backend: &EngineBackend, identity: &Identity, request: CommitDirectoryDrafts,
) -> Result<DirectoryDraftReceipt, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    let owner = authorize_source(&engine, &current, request.source)?;
    for item in &request.selected {
        if item.target.repository != owner { return Err(ApiError::forbidden()); }
        match item.expected {
            DirectoryDraftExpectation::Born { .. } => {
                file_draft::authorize(&mut engine, &current, &item.target)?;
            }
            DirectoryDraftExpectation::Unborn { .. } => {
                crate::engine_backend::unborn::authorize(&engine, &current, &item.target)?;
            }
        }
    }
    let principal = current.principal.parse().map_err(ApiError::invalid_authenticated_principal)?;
    engine.commit_directory_drafts(&request, &principal).map_err(error)
}

pub(in crate::engine_backend) fn read(
    backend: &EngineBackend, identity: &Identity, lookup: DirectoryDraftLookup,
) -> Result<DirectoryDraftReceipt, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    management(&current)?;
    // Original owner admission precedes any operation result or scope detail.
    operations::authorized_record(&engine, &current, lookup.operation_id)?;
    authorize_source(&engine, &current, lookup.source)?;
    let principal = current.principal.parse().map_err(ApiError::invalid_authenticated_principal)?;
    engine.directory_draft_receipt(lookup, &principal).map_err(error)
}
