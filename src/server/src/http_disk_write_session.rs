//! Decode a fresh disk workspace request without inventing an operation receipt.
use crate::{
    ApiError, AppState, CreateDiskWriteSession, auth::Identity,
    backend_disk_write_session::DiskWriteSessionResponse, http_boundary::ApiPath,
};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};
fn parse(mut body: Map<String, Value>) -> Result<CreateDiskWriteSession, ApiError> {
    if let Some(key) = body
        .keys()
        .find(|key| !["slot_id", "path", "branch_id"].contains(&key.as_str()))
    {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let Some(Value::String(slot_id)) = body.remove("slot_id") else {
        unreachable!("invariant: with_id binds the disk slot")
    };
    let path = match body.remove("path") {
        Some(Value::String(path)) => {
            name_policy::PortablePath::parse(&path).map_err(ApiError::invalid_source)?
        }
        None => return Err(ApiError::invalid("file path required")),
        Some(
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) | Value::Object(_),
        ) => return Err(ApiError::invalid("path must be a string")),
    };
    let branch_id = match body.remove("branch_id") {
        None => None,
        Some(Value::String(branch)) => Some(branch.try_into().map_err(ApiError::invalid_source)?),
        Some(
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) | Value::Object(_),
        ) => return Err(ApiError::invalid("branch_id must be a string")),
    };
    Ok(CreateDiskWriteSession {
        slot_id,
        path,
        branch_id,
    })
}
pub(crate) async fn submit(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(slot): ApiPath<String>,
    request: Request,
) -> Result<Json<DiskWriteSessionResponse>, ApiError> {
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
        .run(move |backend| backend.create_disk_write_session(&identity, parse(body)?))
        .await
        .map(DiskWriteSessionResponse::from)
        .map(Json)
}
