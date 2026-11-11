//! Resolve ancestry only when a historical merge input is explicitly selected.
use crate::{
    ApiError, AppState,
    http_boundary::{ApiPath, ApiQuery, Authenticated},
};
use data_serialization::{Deserialize, Deserializer};
use http_routing::{extract::State, response::Response};
use storage_contract::{
    file_history_merge::FileHistorySourceRequest,
    history_window::{HistoryFileId, HistoryNodeId, HistoryRoot},
};

/// Contract failures are reported after query decoding and bounded-read admission.
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
    node_id: String,
    history_root: String,
}

pub(crate) struct Query {
    node_id: Result<HistoryNodeId, storage_contract::Error>,
    history_root: Result<HistoryRoot, storage_contract::Error>,
}

impl<'de> Deserialize<'de> for Query {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = QueryFields::deserialize(deserializer)?;
        Ok(Self {
            node_id: HistoryNodeId::parse(&fields.node_id),
            history_root: HistoryRoot::parse(&fields.history_root),
        })
    }
}

impl Query {
    fn request(self, file: FilePath) -> Result<FileHistorySourceRequest, ApiError> {
        Ok(FileHistorySourceRequest {
            file_id: file.0.map_err(ApiError::contract_input)?,
            node_id: self.node_id.map_err(ApiError::contract_input)?,
            history_root: self.history_root.map_err(ApiError::contract_input)?,
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

#[cfg(test)]
#[path = "../../../test/server/unit/http_file_history_source.rs"]
mod tests;
