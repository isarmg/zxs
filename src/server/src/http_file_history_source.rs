//! Resolve ancestry only when a historical merge input is explicitly selected.
use crate::{
    ApiError, AppState,
    auth::Identity,
    http_boundary::{ApiPath, ApiQuery},
};
use data_serialization::Deserialize;
use http_routing::{Extension, extract::State, response::Response};
use storage_contract::file_history_merge::FileHistorySourceRequest;
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Query {
    node_id: String,
    history_root: String,
}
pub(crate) async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    ApiQuery(query): ApiQuery<Query>,
) -> Result<Response, ApiError> {
    crate::bounded_history_http::read(state, move |backend, cancellation| {
        let request = FileHistorySourceRequest::parse(&file, &query.node_id, &query.history_root)
            .map_err(ApiError::contract_input)?;
        let source = backend.file_history_source(&identity, request.clone(), cancellation)?;
        source.validate(&request).map_err(|source| {
            ApiError::new(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::StateInvalid,
                "historical source backend returned an invalid result",
            )
            .with_source(source)
        })?;
        crate::history_window_encode::encode_bounded(&source, 64 * 1024)
    })
    .await
}
