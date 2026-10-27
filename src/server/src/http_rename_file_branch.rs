//! HTTP decoding for public file-branch label changes.
use crate::{ApiError, AppState, RenameFileBranch, auth::Identity, http_boundary::ApiPath};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};

fn parse(mut body: Map<String, Value>) -> Result<RenameFileBranch, ApiError> {
    if let Some(key) = body
        .keys()
        .find(|key| !["file_id", "branch_id", "name", "operation_id"].contains(&key.as_str()))
    {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let Some(Value::String(file_id)) = body.remove("file_id") else {
        unreachable!("invariant: with_id binds the file_id path")
    };
    let Some(Value::String(branch_id)) = body.remove("branch_id") else {
        unreachable!("invariant: with_id binds the branch_id path")
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
    Ok(RenameFileBranch {
        file_id,
        branch_id,
        name,
        operation_id,
    })
}

pub(crate) async fn rename(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath((file, branch)): ApiPath<(String, String)>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let body = crate::with_id(crate::json_body(request).await?, "file_id", file)?;
    let Value::Object(body) = crate::with_id(body, "branch_id", branch)? else {
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
            .rename_file_branch(&identity, parse(body)?)
            .and_then(crate::operation_wire::operation_wire)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_rename_file_branch.rs"]
mod tests;
