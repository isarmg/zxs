//! Parse mkdir before any lazy root can be published.
use crate::{ApiError, AppState, CreateDiskDirectory, auth::Identity, http_boundary::ApiPath};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};
fn parse(mut body: Map<String, Value>) -> Result<CreateDiskDirectory, ApiError> {
    if let Some(key) = body
        .keys()
        .find(|key| !["slot_id", "kind", "path", "operation_id"].contains(&key.as_str()))
    {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let Some(Value::String(slot_id)) = body.remove("slot_id") else {
        unreachable!("invariant: with_id binds the disk slot")
    };
    if !matches!(body.remove("kind"), Some(Value::String(kind)) if kind == "mkdir") {
        return Err(ApiError::invalid(
            "only mkdir is supported; use identity-bound namespace-batch for existing entries",
        ));
    }
    let operation = match body.remove("operation_id") {
        Some(Value::String(value)) if !value.is_empty() => value,
        _ => return Err(ApiError::invalid("missing string field operation_id")),
    };
    let operation_id = operation.parse().map_err(ApiError::invalid_source)?;
    let path = match body.remove("path") {
        None => return Err(ApiError::invalid("disk namespace root cannot be changed")),
        Some(Value::String(path)) => {
            name_policy::PortablePath::parse(&path).map_err(ApiError::invalid_source)?
        }
        Some(
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) | Value::Object(_),
        ) => return Err(ApiError::invalid("path must be a string")),
    };
    Ok(CreateDiskDirectory {
        slot_id,
        path,
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
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            backend
                .create_disk_directory(&identity, parse(body)?)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}
