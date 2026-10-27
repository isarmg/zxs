//! Decode repository creation once and emit its immutable creation record.
use crate::{
    ApiError, AppState, CreateRepository, auth::Identity, backend_read::RepositoryResponse,
};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};
fn parse(mut body: Map<String, Value>) -> Result<CreateRepository, ApiError> {
    if let Some(key) = body.keys().find(|key| {
        !["name", "primary_slot_id", "target_copies", "operation_id"].contains(&key.as_str())
    }) {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let target = body
        .remove("target_copies")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| ApiError::invalid("target_copies must be a positive integer"))?;
    let target_copies = u32::try_from(target).map_err(ApiError::invalid_source)?;
    let mut field = |key: &str| match body.remove(key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value),
        _ => Err(ApiError::invalid(format!("missing string field {key}"))),
    };
    let name = field("name")?;
    let primary_slot_id = field("primary_slot_id")?;
    let operation_id = field("operation_id")?.parse().map_err(contract_input)?;
    let name = storage_contract::RepositoryName::parse(&name).map_err(contract_input)?;
    Ok(CreateRepository {
        name,
        primary_slot_id,
        target_copies,
        operation_id,
    })
}
fn contract_input(source: storage_contract::Error) -> ApiError {
    ApiError::new(
        http::StatusCode::BAD_REQUEST,
        source.code(),
        source.message().into_owned(),
    )
    .with_source(source)
}
pub(crate) async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    request: Request,
) -> Result<Json<RepositoryResponse>, ApiError> {
    let body = crate::json_body(request).await?;
    let Value::Object(body) = body else {
        return Err(ApiError::invalid("expected object"));
    };
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        state
            .backend
            .create_repository(&identity, parse(body)?)
            .map(RepositoryResponse::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
#[cfg(test)]
#[path = "../../../test/server/unit/http_create_repository.rs"]
mod tests;
