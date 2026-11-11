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

fn parse(body: Map<String, Value>) -> Result<MutateDiskDirectory, ApiError> {
    parse_with_source(body, |source| {
        let source: storage_contract::directory_git::DirectoryBranchCreateSource =
            json_codec::from_value(source).map_err(ApiError::invalid_source)?;
        source.validate().map_err(ApiError::invalid_source)?;
        Ok(DiskDirectoryAction::ReviewedCreate(source))
    })
}

fn parse_history(body: Map<String, Value>) -> Result<MutateDiskDirectory, ApiError> {
    parse_with_source(body, |source| {
        json_codec::from_value(source)
            .map(DiskDirectoryAction::HistoricalCreate)
            .map_err(ApiError::invalid_source)
    })
}

fn parse_with_source(
    mut body: Map<String, Value>,
    source: impl FnOnce(Value) -> Result<DiskDirectoryAction, ApiError>,
) -> Result<MutateDiskDirectory, ApiError> {
    if let Some(key) = body
        .keys()
        .find(|key| !["slot_id", "path", "name", "operation_id", "source"].contains(&key.as_str()))
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
    let action = source(
        body.remove("source")
            .ok_or_else(|| ApiError::invalid("missing directory branch source"))?,
    )?;
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
    parse: fn(Map<String, Value>) -> Result<MutateDiskDirectory, ApiError>,
) -> Result<Json<Value>, ApiError> {
    let Value::Object(body) = crate::with_id(crate::json_body(request).await?, "slot_id", slot)?
    else {
        unreachable!("invariant: with_id binds an object body")
    };
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .mutate_disk_directory(&identity, parse(body)?)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}
pub(crate) async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(slot): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    submit(state, identity, slot, request, parse).await
}

pub(crate) async fn create_from_history(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(slot): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    submit(state, identity, slot, request, parse_history).await
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_disk_directory.rs"]
mod tests;
