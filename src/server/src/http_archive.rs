//! Decode archive identity once without changing its raw receipt response shape.
use crate::{
    ApiError, AppState, ArchiveBranch, auth::Identity, backend_archive::ArchiveResponse,
    http_boundary::ApiPath,
};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};

fn parse(mut body: Map<String, Value>) -> Result<ArchiveBranch, ApiError> {
    let operation = match body.remove("operation_id") {
        Some(Value::String(value)) if !value.is_empty() => value,
        _ => return Err(ApiError::invalid("missing string field operation_id")),
    };
    if let Some(key) = body.keys().find(|key| key.as_str() != "branch_id") {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let branch_id = match body.remove("branch_id") {
        Some(Value::String(id)) => id,
        _ => unreachable!("invariant: with_id binds the branch path"),
    };
    let operation_id = operation
        .parse::<storage_contract::OperationId>()
        .map_err(|source| {
            ApiError::new(
                http::StatusCode::BAD_REQUEST,
                source.code(),
                source.message().into_owned(),
            )
            .with_source(source)
        })?;
    Ok(ArchiveBranch {
        branch_id,
        operation_id,
    })
}
pub(crate) async fn archive(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(branch_id): ApiPath<String>,
    request: Request,
) -> Result<Json<ArchiveResponse>, ApiError> {
    let body = crate::with_id(crate::json_body(request).await?, "branch_id", branch_id)?;
    let Value::Object(body) = body else {
        unreachable!("invariant: with_id binds an object body")
    };
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| {
            let request = parse(body)?;
            backend
                .archive_branch(&identity, request)
                .map(ArchiveResponse::from)
        })
        .await
        .map(Json)
}
#[cfg(test)]
#[path = "../../../test/server/unit/http_archive.rs"]
mod tests;
