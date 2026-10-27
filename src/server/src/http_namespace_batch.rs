//! Decode an atomic directory batch before entering the typed backend.
use crate::{ApiError, AppState, NamespaceBatch, auth::Identity, http_boundary::ApiPath};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};

fn parse(mut body: Map<String, Value>) -> Result<NamespaceBatch, ApiError> {
    let Some(Value::String(slot_id)) = body.remove("slot_id") else {
        unreachable!("invariant: with_id binds the disk slot")
    };
    let operation = body.remove("operation_id");
    let request = json_codec::from_value(Value::Object(body)).map_err(ApiError::invalid_source)?;
    let operation_id = match operation {
        Some(Value::String(value)) if !value.is_empty() => value,
        _ => return Err(ApiError::invalid("missing string field operation_id")),
    }
    .parse::<storage_contract::OperationId>()
    .map_err(|source| {
        ApiError::new(
            http::StatusCode::BAD_REQUEST,
            source.code(),
            source.message().into_owned(),
        )
        .with_source(source)
    })?;
    Ok(NamespaceBatch {
        slot_id,
        request,
        operation_id,
    })
}

pub(crate) async fn submit(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(slot): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let Value::Object(body) = crate::with_id(crate::json_body(request).await?, "slot_id", slot)?
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
        state
            .backend
            .namespace_batch(&identity, parse(body)?)
            .and_then(crate::operation_wire::operation_wire)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
