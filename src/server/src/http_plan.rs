//! HTTP plans preserve their raw plan and permanent receipt shapes.
use crate::{
    ApiError, AppState, CancelPlan, CommitPlan,
    auth::Identity,
    backend_plan::PlanResponse,
    http_boundary::{ApiPath, ApiQuery},
};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};
use std::collections::BTreeMap;

fn parse(mut body: Map<String, Value>) -> Result<CancelPlan, ApiError> {
    if let Some(key) = body
        .keys()
        .find(|key| !["plan_id", "operation_id"].contains(&key.as_str()))
    {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let Some(Value::String(plan_id)) = body.remove("plan_id") else {
        unreachable!("invariant: with_id binds the plan path")
    };
    let operation_id = match body.remove("operation_id") {
        Some(Value::String(value)) if !value.is_empty() => operation_id(&value),
        _ => Err(ApiError::invalid("missing string field operation_id")),
    };
    Ok(CancelPlan {
        plan_id,
        operation_id,
    })
}

fn operation_id(value: &str) -> Result<storage_contract::OperationId, ApiError> {
    value.parse().map_err(|source: storage_contract::Error| {
        ApiError::new(
            http::StatusCode::BAD_REQUEST,
            source.code(),
            source.message().into_owned(),
        )
        .with_source(source)
    })
}

fn parse_commit(mut body: Map<String, Value>) -> Result<CommitPlan, ApiError> {
    let Some(Value::String(plan_id)) = body.remove("plan_id") else {
        unreachable!("invariant: with_id binds the plan path")
    };
    let mut field = |key: &str| match body.remove(key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value),
        _ => Err(ApiError::invalid(format!("missing string field {key}"))),
    };
    let operation = field("operation_id")?;
    let plan_digest = field("plan_digest")?;
    if let Some(key) = body.keys().next() {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    Ok(CommitPlan {
        plan_id,
        plan_digest,
        operation_id: operation_id(&operation),
    })
}

pub(crate) async fn commit(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(plan): ApiPath<String>,
    request: Request,
) -> Result<Json<storage_contract::OperationResult>, ApiError> {
    let Value::Object(body) = crate::with_id(crate::json_body(request).await?, "plan_id", plan)?
    else {
        unreachable!("invariant: with_id binds an object body")
    };
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _auth_owner = state.auth;
        state.backend.commit_plan(&identity, parse_commit(body))
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}

pub(crate) async fn info(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(plan): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<PlanResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _auth_owner = state.auth;
        state.backend.plan(&identity, &plan).map(PlanResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}

pub(crate) async fn cancel(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(plan): ApiPath<String>,
    request: Request,
) -> Result<Json<storage_contract::OperationResult>, ApiError> {
    let Value::Object(body) = crate::with_id(crate::json_body(request).await?, "plan_id", plan)?
    else {
        unreachable!("invariant: with_id binds an object body")
    };
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _auth_owner = state.auth;
        state.backend.cancel_plan(&identity, parse(body))
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_plan.rs"]
mod tests;
