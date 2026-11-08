//! Administrator-only discovery of retained files absent from current namespaces.
use crate::{ApiError, AppState, auth::Identity, http_boundary::ApiQuery};
use data_serialization::Deserialize;
use http_routing::{Extension, extract::State, response::Response};
use storage_contract::{detached_history::{MAX_RESPONSE_BYTES, Request}, history_window::{HistoryFileId, HistoryRoot}};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Query { snapshot: Option<String>, after_file: Option<String> }
pub(crate) async fn read(
    State(state): State<AppState>, Extension(identity): Extension<Identity>, ApiQuery(query): ApiQuery<Query>,
) -> Result<Response, ApiError> {
    let request = Request {
        snapshot: query.snapshot.as_deref().map(HistoryRoot::parse).transpose().map_err(ApiError::contract_input)?,
        after_file: query.after_file.as_deref().map(HistoryFileId::parse).transpose().map_err(ApiError::contract_input)?,
    };
    request.validate().map_err(ApiError::contract_input)?;
    crate::bounded_history_http::read(state, move |backend, cancellation| {
        let page = backend.detached_history(&identity, request.clone(), cancellation)?;
        page.validate(&request).map_err(ApiError::invalid_source)?;
        crate::history_window_encode::encode_bounded(&page, MAX_RESPONSE_BYTES)
    }).await
}
