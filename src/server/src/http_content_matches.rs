//! Bounded metadata matching, with no association or content-read side effects.
use crate::{
    ApiError, AppState,
    auth::Identity,
    http_boundary::{ApiPath, ApiQuery},
};
use data_serialization::Deserialize;
use http_routing::{Extension, extract::State, response::Response};
use storage_contract::content_matches::{ContentMatchRequest, MAX_CONTENT_MATCH_RESPONSE_BYTES};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Query {
    node_id: String,
    history_root: String,
    after_version_id: Option<String>,
    limit: usize,
}
pub(crate) async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    ApiQuery(query): ApiQuery<Query>,
) -> Result<Response, ApiError> {
    crate::bounded_history_http::read(state, move |backend, cancellation| {
        let request = ContentMatchRequest::parse(
            &file,
            &query.node_id,
            &query.history_root,
            query.after_version_id.as_deref(),
            query.limit,
        )
        .map_err(ApiError::contract_input)?;
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
