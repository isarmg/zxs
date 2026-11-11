//! Bounded metadata matching, with no association or content-read side effects.
use crate::{
    ApiError, AppState,
    http_boundary::{ApiPath, ApiQuery, Authenticated},
};
use data_serialization::Deserialize;
use http_routing::{
    extract::{FromRequestParts, State},
    http::request::Parts,
    response::Response,
};
use storage_contract::content_matches::{ContentMatchRequest, MAX_CONTENT_MATCH_RESPONSE_BYTES};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Query {
    node_id: String,
    history_root: String,
    after_version_id: Option<String>,
    limit: usize,
}

/// Keep contract errors until bounded read admission has reserved capacity.
pub(crate) struct RequestInput(storage_contract::Result<ContentMatchRequest>);

impl<S: Send + Sync> FromRequestParts<S> for RequestInput {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let ApiPath(file) = ApiPath::<String>::from_request_parts(parts, state).await?;
        let ApiQuery(query) = ApiQuery::<Query>::from_request_parts(parts, state).await?;
        Ok(Self(ContentMatchRequest::parse(
            &file,
            &query.node_id,
            &query.history_root,
            query.after_version_id.as_deref(),
            query.limit,
        )))
    }
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    RequestInput(request): RequestInput,
) -> Result<Response, ApiError> {
    crate::bounded_history_http::read(state, move |backend, cancellation| {
        let request = request.map_err(ApiError::contract_input)?;
        let page = backend.content_matches(&identity, request.clone(), cancellation)?;
        page.validate(&request).map_err(|source| {
            ApiError::new(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::StateInvalid,
                "content match backend returned an invalid page",
            )
            .with_source(source)
        })?;
        crate::history_window_encode::encode_bounded(&page, MAX_CONTENT_MATCH_RESPONSE_BYTES)
    })
    .await
}
