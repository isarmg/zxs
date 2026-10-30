//! Decode user notes once; missing/null is not an explicit empty clear.
use crate::{ApiError, AppState, NoteTarget, UpdateNote, auth::Identity, http_boundary::ApiPath};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};
#[derive(Clone, Copy)]
enum NoteRoute {
    File,
    Version,
    Node,
}
fn bound(body: &mut Map<String, Value>, key: &str) -> String {
    let Some(Value::String(id)) = body.remove(key) else {
        unreachable!("invariant: with_id binds every note target path")
    };
    id
}
fn parse(mut body: Map<String, Value>, route: NoteRoute) -> Result<UpdateNote, ApiError> {
    let keys: &[&str] = match route {
        NoteRoute::File => &["file_id", "note", "operation_id"],
        NoteRoute::Version => &["version_id", "note", "operation_id"],
        NoteRoute::Node => &["file_id", "node_id", "note", "operation_id"],
    };
    if let Some(key) = body.keys().find(|key| !keys.contains(&key.as_str())) {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let target = match route {
        NoteRoute::File => NoteTarget::File(bound(&mut body, "file_id")),
        NoteRoute::Version => NoteTarget::Version(bound(&mut body, "version_id")),
        NoteRoute::Node => NoteTarget::Node {
            file: bound(&mut body, "file_id"),
            node: bound(&mut body, "node_id"),
        },
    };
    let note = match body.remove("note") {
        Some(Value::String(value)) => value,
        _ => return Err(ApiError::invalid("note must be a string")),
    };
    let operation = match body.remove("operation_id") {
        Some(Value::String(value)) if !value.is_empty() => value,
        _ => return Err(ApiError::invalid("missing string field operation_id")),
    };
    let operation_id = operation.parse().map_err(ApiError::invalid_source)?;
    let note = storage_contract::Note::parse(note).map_err(ApiError::contract_input)?;
    Ok(UpdateNote {
        target,
        note,
        operation_id,
    })
}
async fn submit(
    state: AppState,
    identity: Identity,
    body: Value,
    route: NoteRoute,
) -> Result<Json<Value>, ApiError> {
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
                .update_note(&identity, parse(body, route)?)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}
pub(crate) async fn file(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(file): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let body = crate::with_id(crate::json_body(request).await?, "file_id", file)?;
    submit(state, identity, body, NoteRoute::File).await
}
pub(crate) async fn version(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(version): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let body = crate::with_id(crate::json_body(request).await?, "version_id", version)?;
    submit(state, identity, body, NoteRoute::Version).await
}
pub(crate) async fn node(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath((file, node)): ApiPath<(String, String)>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let body = crate::with_id(crate::json_body(request).await?, "file_id", file)?;
    let body = crate::with_id(body, "node_id", node)?;
    submit(state, identity, body, NoteRoute::Node).await
}
#[cfg(test)]
#[path = "../../../test/server/unit/http_note.rs"]
mod tests;
