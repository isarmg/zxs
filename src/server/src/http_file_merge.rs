//! Decode explicit merge consent and request records at the HTTP boundary.
use crate::http_file_metadata::{location, operation, string};
use crate::{ApiError, AppState, MergeFileBranches, auth::Identity, http_boundary::ApiPath};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};
fn parse(mut body: Map<String, Value>) -> Result<MergeFileBranches, ApiError> {
    if let Some(key) = body.keys().find(|key| {
        ![
            "file_id",
            "inputs",
            "chosen_node_id",
            "surviving_branch_id",
            "confirmed",
            "repository_id",
            "path",
            "operation_id",
        ]
        .contains(&key.as_str())
    }) {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let Some(Value::String(file_id)) = body.remove("file_id") else {
        unreachable!("invariant: with_id binds the file identifier")
    };
    let inputs = json_codec::from_value(
        body.remove("inputs")
            .ok_or_else(|| ApiError::invalid("inputs are required"))?,
    )
    .map_err(ApiError::invalid_source)?;
    let chosen_node_id = string(body.remove("chosen_node_id"), "chosen_node_id")?;
    let surviving_branch_id = string(body.remove("surviving_branch_id"), "surviving_branch_id")?;
    let confirmed = matches!(body.remove("confirmed"), Some(Value::Bool(true)));
    let location = location(&mut body)?;
    let operation_id = operation(body.remove("operation_id"))?;
    Ok(MergeFileBranches {
        file_id,
        inputs,
        chosen_node_id,
        surviving_branch_id,
        confirmed,
        location,
        operation_id,
    })
}
pub(crate) async fn submit(
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
                .merge_file_branches(&identity, parse(body)?)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}
