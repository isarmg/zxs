//! Bind ordinary directory protection to the current management identity.
use crate::engine_backend::{EngineBackend, error, repository};
use crate::{ApiError, ConfigureAutomaticDirectoryBackups, auth::Identity};
use storage_contract::{ErrorCode, OperationResult, Principal};

pub(super) fn configure(
    backend: &EngineBackend,
    identity: &Identity,
    request: ConfigureAutomaticDirectoryBackups,
) -> Result<OperationResult, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    if current.smb_binding.is_some() {
        return Err(ApiError::forbidden());
    }
    repository(&engine, &request.repository_id.to_string())?;
    let principal = current
        .principal
        .parse::<Principal>()
        .map_err(ApiError::invalid_authenticated_principal)?;
    // Keep the exact directory and generation in the engine request. Its
    // Existing branch must precede today's namespace and generation checks.
    engine
        .configure_automatic_directory_backups(storage_engine::AutomaticDirectoryBackupRequest {
            repository_id: request.repository_id,
            path: &request.path,
            directory_id: request.directory_id,
            target_copies: request.target_copies,
            expected_generation: request.expected_generation,
            principal: &principal,
            operation_id: request.operation_id,
        })
        .map_err(|failure| match failure.code() {
            ErrorCode::RouteIncomplete | ErrorCode::ScopeChanged => {
                ApiError::conflict(failure.code(), &failure.message()).with_source(failure)
            }
            _ => error(failure),
        })
}
