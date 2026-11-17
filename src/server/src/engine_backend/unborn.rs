//! Same protected account and repository boundary as management Draft requests.
use super::{EngineBackend, error, repository};
use crate::{ApiError, CommitUnbornFile, Identity, UnbornExpectation, UnbornFileSnapshot};
use storage_contract::file_draft::DraftLocation;

pub(super) fn authorize(
    engine: &storage_engine::Engine,
    current: &Identity,
    target: &DraftLocation,
) -> Result<(), ApiError> {
    if current.smb_binding.is_some() { return Err(ApiError::forbidden()); }
    let requested = repository(engine, &target.repository.to_string())?;
    let record = engine.state().file_containers.get(&target.file)
        .ok_or_else(ApiError::forbidden)?;
    if !requested.file_manager_root || record.namespace.repository != target.repository {
        return Err(ApiError::forbidden());
    }
    // Stable resource ownership only: a committed first-history retry no
    // longer has Unborn state. Core must reach Existing before dynamic CAS.
    Ok(())
}

pub(super) fn status(
    backend: &EngineBackend,
    identity: &Identity,
    target: DraftLocation,
) -> Result<UnbornFileSnapshot, ApiError> {
    let (engine, current) = backend.lock_current(identity)?;
    authorize(&engine, &current, &target)?;
    let working = engine.unborn_working_state(&target).map_err(error)?;
    Ok(UnbornFileSnapshot {
        target, expected: UnbornExpectation::from_working(&working),
        mode: working.mode(), logical_size: working.logical_size(),
    })
}

pub(super) fn commit(
    backend: &EngineBackend,
    identity: &Identity,
    request: CommitUnbornFile,
) -> Result<storage_contract::OperationResult, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    authorize(&engine, &current, &request.target)?;
    let principal = current.principal.parse().map_err(ApiError::invalid_authenticated_principal)?;
    engine.commit_unborn_working(
        &request.expected.selection(request.target), &principal,
        request.operation_id, &request.message,
    ).map_err(error)
}
