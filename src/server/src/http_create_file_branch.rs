//! HTTP decoding for public file branches from frozen sources.
use crate::{ApiError, AppState, CreateFileBranch, auth::Identity, http_boundary::ApiPath};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};

fn parse(mut body: Map<String, Value>) -> Result<CreateFileBranch, ApiError> {
    if let Some(key) = body.keys().find(|key| {
        ![
            "file_id",
            "source_version_id",
            "source_node_id",
            "name",
            "operation_id",
        ]
        .contains(&key.as_str())
    }) {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let Some(Value::String(file_id)) = body.remove("file_id") else {
        unreachable!("invariant: with_id binds the file_id path")
    };
    let source_node_id = match body.remove("source_node_id") {
        None => None,
        Some(Value::String(value)) if !value.is_empty() => Some(value),
        Some(
            Value::String(_)
            | Value::Null
            | Value::Bool(_)
            | Value::Number(_)
            | Value::Array(_)
            | Value::Object(_),
        ) => {
            return Err(ApiError::invalid(
                "source_node_id must be a nonempty node ID",
            ));
        }
    };
    let source_version_id = match body.remove("source_version_id") {
        Some(Value::String(value)) if !value.is_empty() => value,
        _ => return Err(ApiError::invalid("missing string field source_version_id")),
    };
    let raw_name = match body.remove("name") {
        Some(Value::String(value)) if !value.is_empty() => value,
        _ => return Err(ApiError::invalid("missing string field name")),
    };
    let operation = match body.remove("operation_id") {
        Some(Value::String(value)) if !value.is_empty() => value,
        _ => return Err(ApiError::invalid("missing string field operation_id")),
    };
    let operation_id = operation.parse().map_err(ApiError::invalid_source)?;
    let name = storage_contract::BranchName::parse(&raw_name).map_err(|source| {
        ApiError::new(
            http::StatusCode::BAD_REQUEST,
            source.code(),
            source.message().into_owned(),
        )
        .with_source(source)
    })?;
    Ok(CreateFileBranch {
        file_id,
        source_version_id,
        source_node_id,
        name,
        operation_id,
    })
}

pub(crate) async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let Value::Object(body) = crate::with_id(crate::json_body(request).await?, "file_id", file)?
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
                .create_file_branch(&identity, parse(body)?)
                .and_then(crate::operation_wire::operation_wire)
        })
        .await
        .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_create_file_branch.rs"]
mod tests;
