//! HTTP operation lookup parsing and final public receipt projection.
use crate::{
    ApiError, AppState,
    auth::Identity,
    http_boundary::{ApiPath, ApiQuery},
};
use http_routing::{Extension, Json, extract::State};
use std::collections::BTreeMap;

pub(crate) fn operation_id(value: &str) -> Result<storage_contract::OperationId, ApiError> {
    value.parse().map_err(|source: storage_contract::Error| {
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
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<json_codec::Value>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            let operation = operation_id(&id)?;
            backend
                .operation(&identity, operation)
                .and_then(crate::operation_wire::operation_observation_wire)
        })
        .await
        .map(Json)
}
pub(crate) async fn operation_outcome(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<json_codec::Value>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            let operation = operation_id(&id)?;
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
