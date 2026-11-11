//! Bounded read-only directory histories retain admission through response delivery.
use crate::{
    ApiError, AppState,
    auth::Identity,
    http_boundary::{ApiPath, ApiQuery, Authenticated},
};
use http_routing::{
    extract::{FromRequestParts, State},
    http::request::Parts,
    response::Response,
};
use std::collections::BTreeMap;
use storage_contract::directory_history::{
    DirectoryHistoryCollection, DirectoryHistoryRequest, MAX_DIRECTORY_HISTORY_RESPONSE_BYTES,
};

type ParsedRequest = storage_contract::Result<DirectoryHistoryRequest>;

async fn extract<S: Send + Sync>(
    parts: &mut Parts,
    state: &S,
    collection: DirectoryHistoryCollection,
) -> Result<ParsedRequest, ApiError> {
    let ApiPath((repository, directory)) =
        ApiPath::<(String, String)>::from_request_parts(parts, state).await?;
    let ApiQuery(query) =
        ApiQuery::<BTreeMap<String, String>>::from_request_parts(parts, state).await?;
    Ok(DirectoryHistoryRequest::parse(
        &repository, &directory, collection, query,
    ))
}

pub(crate) struct NodesInput(ParsedRequest);

impl<S: Send + Sync> FromRequestParts<S> for NodesInput {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        extract(parts, state, DirectoryHistoryCollection::Nodes)
            .await
            .map(Self)
    }
}

pub(crate) struct BranchesInput(ParsedRequest);

impl<S: Send + Sync> FromRequestParts<S> for BranchesInput {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        extract(parts, state, DirectoryHistoryCollection::Branches)
            .await
            .map(Self)
    }
}

async fn read(
    state: AppState,
    identity: Identity,
    request: ParsedRequest,
) -> Result<Response, ApiError> {
    crate::bounded_history_http::read(state, move |backend, cancellation| {
        let request = request.map_err(ApiError::contract_input)?;
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
    Authenticated(identity): Authenticated,
    NodesInput(request): NodesInput,
) -> Result<Response, ApiError> {
    read(state, identity, request).await
}

pub(crate) async fn branches(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    BranchesInput(request): BranchesInput,
) -> Result<Response, ApiError> {
    read(state, identity, request).await
}
