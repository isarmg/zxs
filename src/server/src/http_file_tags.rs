//! Decode tag-target records; the public core boundary owns tag policy checks.
use crate::{
    ApiError, AppState, UpdateFileNodeTags,
    auth::Identity,
    http_boundary::ApiPath,
    http_file_metadata::{location, operation},
};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};
fn parse(mut body: Map<String, Value>) -> Result<UpdateFileNodeTags, ApiError> {
    if let Some(key) = body.keys().find(|key| {
        ![
            "file_id",
            "targets",
            "add",
            "remove",
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
    let location = location(&mut body)?;
    let targets = json_codec::from_value(
        body.remove("targets")
            .ok_or_else(|| ApiError::invalid("targets are required"))?,
    )
    .map_err(ApiError::invalid_source)?;
    let mut strings = |key: &str| -> Result<Vec<String>, ApiError> {
        json_codec::from_value(
            body.remove(key)
                .ok_or_else(|| ApiError::invalid(format!("{key} is required")))?,
        )
        .map_err(ApiError::invalid_source)
    };
    let add = strings("add")?;
    let remove = strings("remove")?;
    let operation_id = operation(body.remove("operation_id"))?;
    Ok(UpdateFileNodeTags {
        file_id,
        targets,
        add,
        remove,
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
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _auth_owner = state.auth;
        state
            .backend
            .update_file_node_tags(&identity, parse(body)?)
            .and_then(crate::operation_wire::operation_wire)
    })
    .await
    .map_err(ApiError::join)?
    .map(Json)
}
