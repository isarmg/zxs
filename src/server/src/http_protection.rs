//! HTTP-only shape parsing and operation identity decoding for policy mutations.
use crate::{
    ApiError, AppState,
    auth::Identity,
    http_boundary::{ApiPath, ApiQuery},
};
use data_serialization::{Deserialize, Deserializer};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};
use std::collections::BTreeMap;

fn nonempty_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        return Err(data_serialization::de::Error::custom(
            "expected nonempty string",
        ));
    }
    Ok(value)
}
// The private fields are decoded only after `fields` rejects every unknown key.
#[derive(Deserialize)]
struct PolicyFields {
    #[serialization(deserialize_with = "nonempty_string")]
    policy_id: String,
    path_prefix: String,
    target_copies: u32,
    #[serialization(with = "storage_contract::u64_string")]
    expected_generation: u64,
    #[serialization(deserialize_with = "nonempty_string")]
    operation_id: String,
}
#[derive(Deserialize)]
struct PlacementFields {
    #[serialization(deserialize_with = "nonempty_string")]
    rule_id: String,
    path_prefix: String,
    #[serialization(deserialize_with = "nonempty_string")]
    primary_slot_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    expected_generation: u64,
    #[serialization(deserialize_with = "nonempty_string")]
    operation_id: String,
}
fn fields(mut body: Map<String, Value>, allowed: &[&str]) -> Result<(String, Value), ApiError> {
    if let Some(key) = body.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let repository = match body.remove("repository_id") {
        Some(Value::String(id)) if !id.is_empty() => id,
        _ => return Err(ApiError::invalid("missing string field repository_id")),
    };
    Ok((repository, Value::Object(body)))
}
fn operation(id: &str) -> Result<storage_contract::OperationId, ApiError> {
    id.parse::<storage_contract::OperationId>()
        .map_err(ApiError::contract_input)
}
fn parse_policy(body: Map<String, Value>) -> Result<crate::UpdateProtectionPolicy, ApiError> {
    let (repository_id, body) = fields(
        body,
        &[
            "policy_id",
            "repository_id",
            "path_prefix",
            "target_copies",
            "expected_generation",
            "operation_id",
        ],
    )?;
    let PolicyFields {
        policy_id,
        path_prefix,
        target_copies,
        expected_generation,
        operation_id,
    } = json_codec::from_value(body).map_err(ApiError::invalid_source)?;
    let operation_id = operation(&operation_id)?;
    Ok(crate::UpdateProtectionPolicy {
        policy_id,
        repository_id,
        path_prefix,
        target_copies,
        expected_generation,
        operation_id,
    })
}
fn parse_placement(body: Map<String, Value>) -> Result<crate::UpdatePlacementRule, ApiError> {
    let (repository_id, body) = fields(
        body,
        &[
            "rule_id",
            "repository_id",
            "path_prefix",
            "primary_slot_id",
            "expected_generation",
            "operation_id",
        ],
    )?;
    let PlacementFields {
        rule_id,
        path_prefix,
        primary_slot_id,
        expected_generation,
        operation_id,
    } = json_codec::from_value(body).map_err(ApiError::invalid_source)?;
    let operation_id = operation(&operation_id)?;
    Ok(crate::UpdatePlacementRule {
        rule_id,
        repository_id,
        path_prefix,
        primary_slot_id,
        expected_generation,
        operation_id,
    })
}
async fn bound_body(
    request: Request,
    key: &str,
    id: String,
) -> Result<Map<String, Value>, ApiError> {
    let value = crate::with_id(crate::json_body(request).await?, key, id)?;
    let Value::Object(body) = value else {
        unreachable!("invariant: with_id binds an object body")
    };
    Ok(body)
}
pub(crate) async fn policy(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<crate::backend_protection::PolicyResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            backend
                .protection_policy(&identity, &id)
                .map(crate::backend_protection::PolicyResponse::from)
        })
        .await
        .map(Json)
}
pub(crate) async fn placement_rule(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(_query): ApiQuery<BTreeMap<String, String>>,
) -> Result<Json<crate::backend_protection::PlacementRuleResponse>, ApiError> {
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            backend
                .placement_rule(&identity, &id)
                .map(crate::backend_protection::PlacementRuleResponse::from)
        })
        .await
        .map(Json)
}
pub(crate) async fn update_policy(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let body = bound_body(request, "policy_id", id).await?;
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            let command = parse_policy(body)?;
            backend
                .update_protection_policy(&identity, command)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}
pub(crate) async fn update_placement_rule(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let body = bound_body(request, "rule_id", id).await?;
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            let command = parse_placement(body)?;
            backend
                .update_placement_rule(&identity, command)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}
#[cfg(test)]
#[path = "../../../test/server/unit/http_protection.rs"]
mod tests;
