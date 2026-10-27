//! Bind repository identity and decode creation inputs before engine admission.
use crate::{
    ApiError, AppState, CreateBranch, auth::Identity, backend_branch::BranchResponse,
    http_boundary::ApiPath,
};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};

fn parse(mut body: Map<String, Value>) -> Result<CreateBranch, ApiError> {
    let mut field = |key: &str| match body.remove(key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value),
        _ => Err(ApiError::invalid(format!("missing string field {key}"))),
    };
    let repository_id = field("repository_id")?;
    let source_commit_id = field("source_commit_id")?;
    let name = field("name")?;
    let operation = field("operation_id")?;
    if let Some(key) = body.keys().next() {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let operation_id = operation.parse().map_err(contract_input)?;
    let name = storage_contract::BranchName::parse(&name).map_err(contract_input)?;
    Ok(CreateBranch {
        repository_id,
        source_commit_id,
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
pub(crate) async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(repository): ApiPath<String>,
    request: Request,
) -> Result<Json<BranchResponse>, ApiError> {
    let body = crate::with_id(
        crate::json_body(request).await?,
        "repository_id",
        repository,
    )?;
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
            backend
                .create_branch(&identity, parse(body)?)
                .map(BranchResponse::from)
        })
        .await
        .map(Json)
}
#[cfg(test)]
#[path = "../../../test/server/unit/http_create_branch.rs"]
mod tests;
