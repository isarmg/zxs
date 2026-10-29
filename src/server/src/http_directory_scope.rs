//! Decode the public directory-initialization command once at HTTP ingress.
use crate::{ApiError, AppState, InitializeDirectoryScope, auth::Identity, http_boundary::ApiPath};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};

fn parse(mut body: Map<String, Value>) -> Result<InitializeDirectoryScope, ApiError> {
    if let Some(key) = body
        .keys()
        .find(|key| !["repository_id", "path", "operation_id"].contains(&key.as_str()))
    {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let repository_id = match body.remove("repository_id") {
        Some(Value::String(id)) => id,
        _ => unreachable!("invariant: with_id binds the repository path to an object body"),
    };
    let path = match body.remove("path") {
        Some(Value::String(path)) if path.is_empty() => None,
        Some(Value::String(path)) => Some(
            name_policy::PortablePath::parse(&path)
                .map_err(ApiError::invalid_source)?
                .into(),
        ),
        _ => return Err(ApiError::invalid("path must be a string")),
    };
    let operation = match body.remove("operation_id") {
        Some(Value::String(value)) if !value.is_empty() => value,
        _ => return Err(ApiError::invalid("missing string field operation_id")),
    };
    let operation_id = operation
        .parse::<storage_contract::OperationId>()
        .map_err(ApiError::contract_input)?;
    Ok(InitializeDirectoryScope {
        repository_id,
        path,
        operation_id,
    })
}

pub(crate) async fn initialize(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let value = crate::with_id(crate::json_body(request).await?, "repository_id", id)?;
    let Value::Object(body) = value else {
        unreachable!("invariant: with_id binds an object body")
    };
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            let command = parse(body)?;
            backend
                .initialize_directory_scope(&identity, command)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_directory_scope.rs"]
mod tests;
