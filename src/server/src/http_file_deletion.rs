//! Authenticated current deletion discovery and exact-node cancellation.
use crate::{
    ApiError, AppState,
    auth::Identity,
    http_boundary::{ApiPath, ApiQuery},
};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
    response::Response,
};
use std::collections::BTreeMap;
use storage_contract::client_protocol::MAX_FILE_DELETION_RESPONSE_BYTES;
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

pub(crate) async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    if !query.is_empty() {
        return Err(ApiError::invalid(
            "deletion read does not accept query fields",
        ));
    }
    let file = HistoryFileId::parse(&file).map_err(ApiError::contract_input)?;
    crate::bounded_history_http::read(state, move |backend, _| {
        crate::history_window_encode::encode_bounded(
            &backend.current_file_deletion(&identity, &file)?,
            MAX_FILE_DELETION_RESPONSE_BYTES,
        )
    })
    .await
}

pub(crate) async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    let request = crate::ReadFileDeletions::parse(query)?;
    crate::bounded_history_http::read(state, move |backend, _| {
        crate::history_window_encode::encode_bounded(
            &backend.file_deletions(&identity, request)?,
            MAX_FILE_DELETION_RESPONSE_BYTES,
        )
    })
    .await
}

const MAX_MUTATION_BYTES: usize = 16 * 1024;

async fn body<T: data_serialization::de::DeserializeOwned>(
    request: Request,
) -> Result<T, ApiError> {
    let bytes = crate::http_boundary::read_body(request, MAX_MUTATION_BYTES).await?;
    storage_contract::decode_strict(&bytes).map_err(ApiError::invalid_source)
}

#[derive(data_serialization::Deserialize)]
#[serialization(deny_unknown_fields)]
struct Delete {
    repository_id: HistoryRepositoryId,
    path: String,
    operation_id: storage_contract::OperationId,
}

pub(crate) async fn submit(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
    request: Request,
) -> Result<Json<json_codec::Value>, ApiError> {
    if !query.is_empty() {
        return Err(ApiError::invalid(
            "file deletion does not accept query fields",
        ));
    }
    let file_id = HistoryFileId::parse(&file).map_err(ApiError::contract_input)?;
    let body: Delete = body(request).await?;
    let path = name_policy::LookupPath::parse(&body.path).map_err(ApiError::invalid_source)?;
    let command = crate::DeleteFile {
        repository_id: body.repository_id,
        file_id,
        path,
        operation_id: body.operation_id,
    };
    mutate(state, move |backend| {
        backend.soft_delete_file(&identity, command)
    })
    .await
}

#[derive(data_serialization::Deserialize)]
#[serialization(deny_unknown_fields)]
struct Undo {
    deletion_node_id: String,
    operation_id: storage_contract::OperationId,
}

pub(crate) async fn undo(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    request: Request,
) -> Result<Json<json_codec::Value>, ApiError> {
    let file_id = HistoryFileId::parse(&file).map_err(ApiError::contract_input)?;
    let body: Undo = body(request).await?;
    storage_contract::validate_id(&body.deletion_node_id, "fdel")
        .map_err(ApiError::contract_input)?;
    mutate(state, move |backend| {
        backend.restore_deleted_file(
            &identity,
            crate::RestoreDeletedFile {
                file_id,
                deletion_node_id: body.deletion_node_id,
                operation_id: body.operation_id,
            },
        )
    })
    .await
}

pub(crate) async fn batch_preview(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(operation): ApiPath<storage_contract::OperationId>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    if !query.is_empty() {
        return Err(ApiError::invalid(
            "batch restore preview does not accept query fields",
        ));
    }
    crate::bounded_history_http::read(state, move |backend, _| {
        crate::history_window_encode::encode_bounded(
            &backend.file_deletion_batch_undo_preview(&identity, operation)?,
            storage_contract::file_deletion_batch::MAX_RESPONSE_BYTES,
        )
    })
    .await
}

pub(crate) async fn undo_batch(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
    request: Request,
) -> Result<Json<json_codec::Value>, ApiError> {
    if !query.is_empty() {
        return Err(ApiError::invalid(
            "file deletion batch undo does not accept query fields",
        ));
    }
    let request: storage_contract::file_deletion_batch::UndoFileDeletionBatchBody =
        body(request).await?;
    mutate(state, move |backend| {
        backend.restore_deleted_file_batch(&identity, &request)
    })
    .await
}

pub(crate) async fn mutate(
    state: AppState,
    work: impl FnOnce(&dyn crate::Backend) -> Result<storage_contract::OperationResult, ApiError>
    + Send
    + 'static,
) -> Result<Json<json_codec::Value>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            work(backend).and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_file_deletion.rs"]
mod tests;

pub(crate) mod projection;
pub(crate) mod review;
