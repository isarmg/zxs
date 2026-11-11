//! Automatic same-history relationships without body reads or identity writes.
use crate::{
    ApiError, AppState,
    http_boundary::{ApiPath, ApiQuery, Authenticated},
};
use data_serialization::{Deserialize, Deserializer};
use http_routing::{extract::State, response::Response};
use storage_contract::{
    history_window::{HistoryFileId, HistoryRoot},
    shared_history::{MAX_SHARED_HISTORY_RESPONSE_BYTES, SharedHistoryRequest},
};

/// Preserve query rejection and capacity admission before contract errors.
pub(crate) struct FilePath(Result<HistoryFileId, storage_contract::Error>);

impl<'de> Deserialize<'de> for FilePath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(Self(HistoryFileId::parse(&value)))
    }
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct QueryFields {
    history_root: String,
}

pub(crate) struct Query(Result<HistoryRoot, storage_contract::Error>);

impl<'de> Deserialize<'de> for Query {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = QueryFields::deserialize(deserializer)?;
        Ok(Self(HistoryRoot::parse(&fields.history_root)))
    }
}

impl Query {
    fn request(self, file: FilePath) -> Result<SharedHistoryRequest, ApiError> {
        Ok(SharedHistoryRequest {
            file_id: file.0.map_err(ApiError::contract_input)?,
            history_root: self.0.map_err(ApiError::contract_input)?,
        })
    }
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<FilePath>,
    ApiQuery(query): ApiQuery<Query>,
) -> Result<Response, ApiError> {
    crate::bounded_history_http::read(state, move |backend, cancellation| {
        let request = query.request(file)?;
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

#[cfg(test)]
#[path = "../../../test/server/unit/http_shared_history.rs"]
mod tests;
