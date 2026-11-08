//! Automatic same-history relationships without body reads or identity writes.
use crate::{
    ApiError, AppState,
    auth::Identity,
    http_boundary::{ApiPath, ApiQuery},
};
use data_serialization::Deserialize;
use http_routing::{Extension, extract::State, response::Response};
use storage_contract::shared_history::{MAX_SHARED_HISTORY_RESPONSE_BYTES, SharedHistoryRequest};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Query {
    history_root: String,
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    ApiQuery(query): ApiQuery<Query>,
) -> Result<Response, ApiError> {
    crate::bounded_history_http::read(state, move |backend, cancellation| {
        let request = SharedHistoryRequest::parse(&file, &query.history_root)
            .map_err(ApiError::contract_input)?;
        let graph = backend.shared_history(&identity, request.clone(), cancellation)?;
        graph.validate(&request).map_err(|source| {
            ApiError::new(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::StateInvalid,
                "shared history backend returned an invalid component",
            )
            .with_source(source)
        })?;
        crate::history_window_encode::encode_bounded(&graph, MAX_SHARED_HISTORY_RESPONSE_BYTES)
    })
    .await
}
