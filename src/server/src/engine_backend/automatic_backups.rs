//! Bind ordinary automatic protection to the current management identity.
use crate::engine_backend::{EngineBackend, error, repository};
use crate::{
    ApiError, ConfigureAutomaticDirectoryBackups, ConfigureAutomaticFileBackups, auth::Identity,
};
use storage_contract::{ErrorCode, OperationResult, Principal, RepositoryId};

pub(super) fn configure(
    backend: &EngineBackend,
    identity: &Identity,
    request: ConfigureAutomaticDirectoryBackups,
) -> Result<OperationResult, ApiError> {
    submit(backend, identity, Request::Directory(request))
}

pub(super) fn configure_file(
    backend: &EngineBackend,
    identity: &Identity,
    request: ConfigureAutomaticFileBackups,
) -> Result<OperationResult, ApiError> {
    submit(backend, identity, Request::File(request))
}

enum Request {
    Directory(ConfigureAutomaticDirectoryBackups),
    File(ConfigureAutomaticFileBackups),
}

impl Request {
    fn repository(&self) -> RepositoryId {
        match self {
            Self::Directory(request) => request.repository_id,
            Self::File(request) => request.repository_id,
        }
    }

    fn publish(
        self,
        engine: &mut storage_engine::Engine,
        principal: &Principal,
    ) -> storage_engine::Result<OperationResult> {
        match self {
            Self::Directory(request) => engine.configure_automatic_directory_backups(
                storage_engine::AutomaticDirectoryBackupRequest {
                    repository_id: request.repository_id,
                    path: &request.path,
                    directory_id: request.directory_id,
                    target_copies: request.target_copies,
                    expected_generation: request.expected_generation,
                    principal,
                    operation_id: request.operation_id,
                },
            ),
            Self::File(request) => engine.configure_automatic_file_backups(
                storage_engine::AutomaticFileBackupRequest {
                    repository_id: request.repository_id,
                    path: &request.path,
                    entry_id: request.entry_id,
                    file_id: request.file_id,
                    target_copies: request.target_copies,
                    expected_generation: request.expected_generation,
                    principal,
                    operation_id: request.operation_id,
                },
            ),
        }
    }
}

fn submit(
    backend: &EngineBackend,
    identity: &Identity,
    request: Request,
) -> Result<OperationResult, ApiError> {
    let (mut engine, current) = backend.lock_current(identity)?;
    if current.smb_binding.is_some() {
        return Err(ApiError::forbidden());
    }
    repository(&engine, &request.repository().to_string())?;
    let principal = current
        .principal
        .parse::<Principal>()
        .map_err(ApiError::invalid_authenticated_principal)?;
    // Exact scope and generation remain in the engine request. Its Existing
    // branch precedes today's namespace and generation checks for both scopes.
    request
        .publish(&mut engine, &principal)
        .map_err(|failure| match failure.code() {
            ErrorCode::RouteIncomplete | ErrorCode::ScopeChanged => {
                ApiError::conflict(failure.code(), &failure.message()).with_source(failure)
            }
            _ => error(failure),
        })
}
