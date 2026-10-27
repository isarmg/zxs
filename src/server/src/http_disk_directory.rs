//! Decode disk-directory mutations before lazy root creation can publish state.
use crate::{
    ApiError, AppState, DiskDirectoryAction, MutateDiskDirectory, auth::Identity,
    http_boundary::ApiPath,
};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};

fn parse(
    mut body: Map<String, Value>,
    action: DiskDirectoryAction,
) -> Result<MutateDiskDirectory, ApiError> {
    if let Some(key) = body
        .keys()
        .find(|key| !["slot_id", "path", "name", "operation_id"].contains(&key.as_str()))
    {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let slot_id = match body.remove("slot_id") {
        Some(Value::String(value)) if !value.is_empty() => value,
        _ => return Err(ApiError::invalid("missing string field slot_id")),
    };
    let path = match body.remove("path") {
        None => None,
        Some(Value::String(path)) if path.is_empty() => None,
        Some(Value::String(path)) => Some(
            name_policy::PortablePath::parse(&path)
                .map_err(ApiError::invalid_source)?
                .into(),
        ),
        Some(
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) | Value::Object(_),
        ) => return Err(ApiError::invalid("path must be a string")),
    };
    let raw_name = match body.remove("name") {
        Some(Value::String(value)) if !value.is_empty() => value,
        _ => return Err(ApiError::invalid("missing string field name")),
    };
    let name = storage_contract::BranchName::parse(&raw_name).map_err(|source| {
        ApiError::new(
            http::StatusCode::BAD_REQUEST,
            source.code(),
            source.message().into_owned(),
        )
        .with_source(source)
    })?;
    let operation = match body.remove("operation_id") {
        Some(Value::String(value)) if !value.is_empty() => value,
        _ => return Err(ApiError::invalid("missing string field operation_id")),
    };
    let operation_id = operation.parse().map_err(ApiError::invalid_source)?;
    Ok(MutateDiskDirectory {
        slot_id,
        path,
        name,
        operation_id,
        action,
    })
}
async fn submit(
    state: AppState,
    identity: Identity,
    slot: String,
    request: Request,
    action: DiskDirectoryAction,
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
            .mutate_disk_directory(&identity, parse(body, action)?)
            .and_then(crate::operation_wire::operation_wire)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
pub(crate) async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(slot): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    submit(state, identity, slot, request, DiskDirectoryAction::Create).await
}
pub(crate) async fn select(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(slot): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    submit(state, identity, slot, request, DiskDirectoryAction::Select).await
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_disk_directory.rs"]
mod tests;
