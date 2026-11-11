//! HTTP operation lookup parsing and final public receipt projection.
use crate::{
    ApiError, AppState,
    http_boundary::{ApiPath, ApiQuery, Authenticated},
};
use data_serialization::{Deserialize, Deserializer};
use http_routing::{Json, extract::State};
use std::collections::BTreeMap;

/// Contract parse failures wait for query and backend-capacity admission.
pub(crate) struct OperationLookupPath(
    Result<storage_contract::OperationId, storage_contract::Error>,
);

impl<'de> Deserialize<'de> for OperationLookupPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(Self(value.parse()))
    }
}

pub(crate) fn operation_id(
    value: OperationLookupPath,
) -> Result<storage_contract::OperationId, ApiError> {
    value.0.map_err(|source| {
        ApiError::new(
            ::http::StatusCode::NOT_FOUND,
            storage_contract::ErrorCode::OperationNotFound,
            "operation not found",
        )
        .with_source(source)
    })
}
pub(crate) async fn operation(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(id): ApiPath<OperationLookupPath>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<json_codec::Value>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            let operation = operation_id(id)?;
            backend
                .operation(&identity, operation)
                .and_then(crate::operation_wire::operation_observation_wire)
        })
        .await
        .map(Json)
}
pub(crate) async fn operation_outcome(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(id): ApiPath<OperationLookupPath>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<json_codec::Value>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            let operation = operation_id(id)?;
            backend
                .operation_outcome(&identity, operation)
                .and_then(crate::operation_wire::operation_wire)
        })
        .await
        .map(Json)
}
#[cfg(test)]
#[path = "../../../test/server/unit/http_operation.rs"]
mod tests;
