//! Bounded read-only directory histories retain admission through response delivery.
use crate::{
    ApiError, AppState,
    auth::Identity,
    http_boundary::{ApiPath, ApiQuery},
};
use http_routing::{Extension, extract::State, response::Response};
use std::collections::BTreeMap;
use storage_contract::directory_history::*;
async fn read(
    state: AppState,
    identity: Identity,
    repository: String,
    directory: String,
    query: BTreeMap<String, String>,
    collection: DirectoryHistoryCollection,
) -> Result<Response, ApiError> {
    crate::bounded_history_http::read(state, move |backend, cancellation| {
        let request = DirectoryHistoryRequest::parse(&repository, &directory, collection, query)
            .map_err(ApiError::contract_input)?;
        let page = backend.directory_history(&identity, request.clone(), cancellation)?;
        page.validate(&request).map_err(|source| {
            ApiError::new(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::StateInvalid,
                "directory history backend returned an invalid page",
            )
            .with_source(source)
        })?;
        crate::history_window_encode::encode_bounded(&page, MAX_DIRECTORY_HISTORY_RESPONSE_BYTES)
    })
    .await
}
pub(crate) async fn nodes(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath((repository, directory)): ApiPath<(String, String)>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    read(
        state,
        identity,
        repository,
        directory,
        query,
        DirectoryHistoryCollection::Nodes,
    )
    .await
}
pub(crate) async fn branches(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath((repository, directory)): ApiPath<(String, String)>,
    ApiQuery(query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    read(
        state,
        identity,
        repository,
        directory,
        query,
        DirectoryHistoryCollection::Branches,
    )
    .await
}
