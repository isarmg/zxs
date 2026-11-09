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
use storage_contract::history_window::HistoryFileId;

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
            1024 * 1024,
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
            1024 * 1024,
        )
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
    let body: Undo = json_codec::from_value(crate::json_body(request).await?)
        .map_err(ApiError::invalid_source)?;
    storage_contract::validate_id(&body.deletion_node_id, "fdel")
        .map_err(ApiError::contract_input)?;
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            backend
                .restore_deleted_file(
                    &identity,
                    crate::RestoreDeletedFile {
                        file_id,
                        deletion_node_id: body.deletion_node_id,
                        operation_id: body.operation_id,
                    },
                )
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}
