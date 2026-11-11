//! Administrator-only discovery of retained files absent from current namespaces.
use crate::{
    ApiError, AppState,
    http_boundary::{ApiQuery, Authenticated},
};
use data_serialization::{Deserialize, Deserializer};
use http_routing::{extract::State, response::Response};
use storage_contract::{
    detached_history::{MAX_RESPONSE_BYTES, Request},
    history_window::{HistoryFileId, HistoryRoot},
};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct QueryFields {
    snapshot: Option<String>,
    after_file: Option<String>,
}

impl QueryFields {
    fn request(self) -> storage_contract::Result<Request> {
        let request = Request {
            snapshot: self
                .snapshot
                .as_deref()
                .map(HistoryRoot::parse)
                .transpose()?,
            after_file: self
                .after_file
                .as_deref()
                .map(HistoryFileId::parse)
                .transpose()?,
        };
        request.validate()?;
        Ok(request)
    }
}

/// Contract failures retain their source and are returned before capacity admission.
pub(crate) struct Query(storage_contract::Result<Request>);

impl<'de> Deserialize<'de> for Query {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = QueryFields::deserialize(deserializer)?;
        Ok(Self(fields.request()))
    }
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiQuery(query): ApiQuery<Query>,
) -> Result<Response, ApiError> {
    let request = query.0.map_err(ApiError::contract_input)?;
    crate::bounded_history_http::read(state, move |backend, cancellation| {
        let page = backend.detached_history(&identity, request.clone(), cancellation)?;
        page.validate(&request).map_err(ApiError::invalid_source)?;
        crate::history_window_encode::encode_bounded(&page, MAX_RESPONSE_BYTES)
    })
    .await
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_detached_history.rs"]
mod tests;
