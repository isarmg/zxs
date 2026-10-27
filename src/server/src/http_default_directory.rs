//! Decode the public directory-selection command once at HTTP ingress.
use crate::{
    ApiError, AppState, SelectDefaultDirectoryBranch, auth::Identity, http_boundary::ApiPath,
};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};

fn parse(mut body: Map<String, Value>) -> Result<SelectDefaultDirectoryBranch, ApiError> {
    if let Some(key) = body.keys().find(|key| {
        !["repository_id", "directory_branch_id", "operation_id"].contains(&key.as_str())
    }) {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let repository_id = match body.remove("repository_id") {
        Some(Value::String(id)) => id,
        _ => unreachable!("invariant: with_id binds the repository path to an object body"),
    };
    let mut field = |key: &str| match body.remove(key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value),
        _ => Err(ApiError::invalid(format!("missing string field {key}"))),
    };
    let directory_branch_id = field("directory_branch_id")?;
    let operation_id = field("operation_id")?
        .parse::<storage_contract::OperationId>()
        .map_err(contract_input)?;
    Ok(SelectDefaultDirectoryBranch {
        repository_id,
        directory_branch_id,
        operation_id,
    })
}

pub(crate) async fn select(
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
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let command = parse(body)?;
        state
            .backend
            .select_default_directory_branch(&identity, command)
            .and_then(crate::operation_wire::operation_wire)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}

fn parse_rename(mut body: Map<String, Value>) -> Result<crate::RenameDirectoryBranch, ApiError> {
    if let Some(key) = body.keys().find(|key| {
        ![
            "repository_id",
            "directory_branch_id",
            "name",
            "operation_id",
        ]
        .contains(&key.as_str())
    }) {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let repository_id = match body.remove("repository_id") {
        Some(Value::String(id)) => id,
        _ => unreachable!("invariant: with_id binds the repository path"),
    };
    let directory_branch_id = match body.remove("directory_branch_id") {
        Some(Value::String(id)) => id,
        _ => unreachable!("invariant: with_id binds the directory branch path"),
    };
    let mut field = |key: &str| match body.remove(key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value),
        _ => Err(ApiError::invalid(format!("missing string field {key}"))),
    };
    let name = field("name")?;
    // The original boundary decoded operation identity before core name validation.
    let operation_id = field("operation_id")?
        .parse::<storage_contract::OperationId>()
        .map_err(contract_input)?;
    let name = storage_contract::BranchName::parse(&name).map_err(contract_input)?;
    Ok(crate::RenameDirectoryBranch {
        repository_id,
        directory_branch_id,
        name,
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
pub(crate) async fn rename(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath((repository_id, directory_branch_id)): ApiPath<(String, String)>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let body = crate::with_id(
        crate::json_body(request).await?,
        "repository_id",
        repository_id,
    )?;
    let body = crate::with_id(body, "directory_branch_id", directory_branch_id)?;
    let Value::Object(body) = body else {
        unreachable!("invariant: with_id binds an object body")
    };
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let command = parse_rename(body)?;
        state
            .backend
            .rename_directory_branch(&identity, command)
            .and_then(crate::operation_wire::operation_wire)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_default_directory.rs"]
mod tests;
