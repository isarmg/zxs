//! Exact native file and directory commands, separate from custom prefixes.
use crate::http_boundary::{ApiJson, ApiPath, Authenticated, JsonBody};
use crate::{
    ApiError, AppState, ConfigureAutomaticDirectoryBackups, ConfigureAutomaticFileBackups,
};
use data_serialization::Deserialize;
use http_routing::{Json, extract::State};
use name_policy::PortablePath;
use storage_contract::{EntryId, FileId, Generation, OperationId, RepositoryId};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Body {
    path: PortablePath,
    directory_id: EntryId,
    target_copies: u32,
    expected_generation: Generation,
    operation_id: OperationId,
}

impl JsonBody for Body {
    fn from_json(value: json_codec::Value) -> Result<Self, ApiError> {
        if !value.is_object() {
            return Err(ApiError::invalid(
                "automatic directory backup requires a named object",
            ));
        }
        json_codec::from_value(value).map_err(ApiError::invalid_source)
    }
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct FileBody {
    path: PortablePath,
    entry_id: EntryId,
    file_id: FileId,
    target_copies: u32,
    expected_generation: Generation,
    operation_id: OperationId,
}

impl JsonBody for FileBody {
    fn from_json(value: json_codec::Value) -> Result<Self, ApiError> {
        if !value.is_object() {
            return Err(ApiError::invalid(
                "automatic file backup requires a named object",
            ));
        }
        json_codec::from_value(value).map_err(ApiError::invalid_source)
    }
}

pub(crate) async fn configure_file(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(repository_id): ApiPath<RepositoryId>,
    ApiJson(body): ApiJson<FileBody>,
) -> Result<Json<json_codec::Value>, ApiError> {
    let request = ConfigureAutomaticFileBackups {
        repository_id,
        path: body.path,
        entry_id: body.entry_id,
        file_id: body.file_id,
        target_copies: body.target_copies,
        expected_generation: body.expected_generation,
        operation_id: body.operation_id,
    };
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .configure_automatic_file_backups(&identity, request)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}

pub(crate) async fn configure(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(repository_id): ApiPath<RepositoryId>,
    ApiJson(body): ApiJson<Body>,
) -> Result<Json<json_codec::Value>, ApiError> {
    let request = ConfigureAutomaticDirectoryBackups {
        repository_id,
        path: body.path,
        directory_id: body.directory_id,
        target_copies: body.target_copies,
        expected_generation: body.expected_generation,
        operation_id: body.operation_id,
    };
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .configure_automatic_directory_backups(&identity, request)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}

#[cfg(test)]
#[path = "../../../../test/server/unit/http_backup_configuration/automatic.rs"]
mod tests;
