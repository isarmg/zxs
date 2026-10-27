//! Decode a public merge relation without changing its immutable replay response.
use crate::{
    ApiError, AppState, CreateMergeLink, auth::Identity, backend_graph::MergeLinkRecord,
    http_boundary::ApiPath,
};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};
fn parse(mut body: Map<String, Value>) -> Result<CreateMergeLink, ApiError> {
    let mut field = |key: &str| match body.remove(key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value),
        _ => Err(ApiError::invalid(format!("missing string field {key}"))),
    };
    let history_tree_id = field("history_tree_id")?;
    let source_branch_id = field("source_branch_id")?;
    let source_commit_id = field("source_commit_id")?;
    let target_branch_id = field("target_branch_id")?;
    let target_commit_id = field("target_commit_id")?;
    let operation = field("operation_id")?;
    if let Some(key) = body.keys().next() {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
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
    Ok(CreateMergeLink {
        history_tree_id,
        source_branch_id,
        source_commit_id,
        target_branch_id,
        target_commit_id,
        operation_id,
    })
}
pub(crate) async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(history): ApiPath<String>,
    request: Request,
) -> Result<Json<MergeLinkRecord>, ApiError> {
    let body = crate::with_id(crate::json_body(request).await?, "history_tree_id", history)?;
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
        state
            .backend
            .create_merge_link(&identity, parse(body)?)
            .map(MergeLinkRecord::from)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
#[cfg(test)]
#[path = "../../../test/server/unit/http_merge_link.rs"]
mod tests;
