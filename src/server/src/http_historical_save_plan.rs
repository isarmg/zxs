//! Decode historical save options once, retaining post-admission field error priority.
use crate::{ApiError, AppState, PrepareHistoricalSave, auth::Identity, http_boundary::ApiPath};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};
fn parse(mut body: Map<String, Value>) -> Result<PrepareHistoricalSave, ApiError> {
    if let Some(key) = body.keys().find(|key| {
        ![
            "history_tree_id",
            "source_commit_id",
            "name",
            "workspace_id",
            "primary_slot_id",
            "target_copies",
        ]
        .contains(&key.as_str())
    }) {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let Some(Value::String(history_tree_id)) = body.remove("history_tree_id") else {
        unreachable!("invariant: with_id binds the history path")
    };
    let options = json_codec::from_value(Value::Object(body)).map_err(ApiError::invalid_source);
    Ok(PrepareHistoricalSave {
        history_tree_id,
        options,
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
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _auth_owner = state.auth;
        state
            .backend
            .prepare_historical_save(&identity, parse(body))
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
