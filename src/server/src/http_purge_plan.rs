//! Decode purge selection once without advancing it ahead of history admission.
use crate::{ApiError, AppState, PreparePurge, auth::Identity, http_boundary::ApiPath};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};
fn parse(mut body: Map<String, Value>) -> Result<PreparePurge, ApiError> {
    if let Some(key) = body
        .keys()
        .find(|key| !["history_tree_id", "version_ids"].contains(&key.as_str()))
    {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let Some(Value::String(history_tree_id)) = body.remove("history_tree_id") else {
        unreachable!("invariant: with_id binds the history path")
    };
    let version_ids = body
        .remove("version_ids")
        .ok_or_else(|| ApiError::invalid("version_ids required"))
        .and_then(|value| json_codec::from_value(value).map_err(ApiError::invalid_source));
    Ok(PreparePurge {
        history_tree_id,
        version_ids,
    })
}
pub(crate) async fn prepare(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(history): ApiPath<String>,
    request: Request,
) -> Result<Json<storage_contract::Plan>, ApiError> {
    let Value::Object(body) =
        crate::with_id(crate::json_body(request).await?, "history_tree_id", history)?
    else {
        unreachable!("invariant: with_id binds an object body")
    };
    let permit = state
        .backend_slots
        .clone()
        .try_acquire_owned()
        .map_err(|source| ApiError::limit("HTTP backend capacity reached").with_source(source))?;
    crate::http_job::HttpBackendJob::new(state.backend, state.auth, permit)
        .run(move |backend| backend.prepare_purge(&identity, parse(body)))
        .await
        .map(Json)
}
