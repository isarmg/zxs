//! Bounded discovery and exact original-node cancellation of one directory D.
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
use storage_contract::directory_git::UndoDirectoryDeletionBody;

fn owner_ids(repository: &str, owner: &str) -> Result<(), ApiError> {
    storage_contract::validate_id(repository, "repo").map_err(ApiError::contract_input)?;
    if owner != repository {
        storage_contract::validate_id(owner, "entry").map_err(ApiError::contract_input)?;
    }
    Ok(())
}

fn ids(repository: &str, owner: &str, directory: &str) -> Result<(), ApiError> {
    owner_ids(repository, owner)?;
    storage_contract::validate_id(directory, "entry").map_err(ApiError::contract_input)
}

pub(crate) async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath((repository, owner)): ApiPath<(String, String)>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    owner_ids(&repository, &owner)?;
    let request = crate::ReadDirectoryDeletions::parse(query)?;
    crate::bounded_history_http::read(state, move |backend, _| {
        crate::history_window_encode::encode_bounded(
            &backend.directory_deletions(&identity, &repository, &owner, request)?,
            MAX_FILE_DELETION_RESPONSE_BYTES,
        )
    })
    .await
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath((repository, owner, directory)): ApiPath<(String, String, String)>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    if !query.is_empty() {
        return Err(ApiError::invalid(
            "directory deletion read has no query fields",
        ));
    }
    ids(&repository, &owner, &directory)?;
    crate::bounded_history_http::read(state, move |backend, _| {
        crate::history_window_encode::encode_bounded(
            &backend.current_directory_deletion(&identity, &repository, &owner, &directory)?,
            MAX_FILE_DELETION_RESPONSE_BYTES,
        )
    })
    .await
}

pub(crate) async fn undo(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(repository): ApiPath<String>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
    request: Request,
) -> Result<Json<json_codec::Value>, ApiError> {
    if !query.is_empty() {
        return Err(ApiError::invalid(
            "directory deletion undo has no query fields",
        ));
    }
    let bytes = crate::http_boundary::read_body(request, 16 * 1024).await?;
    let body: UndoDirectoryDeletionBody =
        storage_contract::decode_strict(&bytes).map_err(ApiError::invalid_source)?;
    ids(&repository, &body.owner_directory_id, &body.directory_id)?;
    storage_contract::validate_id(&body.deletion_node_id, "ddel")
        .map_err(ApiError::contract_input)?;
    crate::http_file_deletion::mutate(state, move |backend| {
        backend.restore_deleted_directory(&identity, &repository, &body)
    })
    .await
}
