//! Decode user notes once; missing/null is not an explicit empty clear.
use crate::{
    ApiError, AppState, NoteTarget, UpdateNote,
    auth::Identity,
    http_boundary::{ApiJson, ApiPath, Authenticated, JsonBody},
};
use http_routing::{Json, extract::State};
use json_codec::{Map, Value};

pub(crate) struct NoteBody(Map<String, Value>);

impl JsonBody for NoteBody {
    fn from_json(value: Value) -> Result<Self, ApiError> {
        match value {
            Value::Object(body) => Ok(Self(body)),
            _ => Err(ApiError::invalid("expected JSON object")),
        }
    }
}

impl NoteBody {
    fn matches_path(&self, key: &str, id: &str) -> Result<(), ApiError> {
        if self.0.get(key).is_some_and(|value| value.as_str() != Some(id)) {
            return Err(ApiError::invalid("path and body identity disagree"));
        }
        Ok(())
    }
}

fn parse(NoteBody(mut body): NoteBody, target: NoteTarget) -> Result<UpdateNote, ApiError> {
    let keys: &[&str] = match &target {
        NoteTarget::File(_) => &["file_id", "note", "operation_id"],
        NoteTarget::Version(_) => &["version_id", "note", "operation_id"],
        NoteTarget::Node { .. } => &["file_id", "node_id", "note", "operation_id"],
    };
    if let Some(key) = body.keys().find(|key| !keys.contains(&key.as_str())) {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
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
    body: NoteBody,
    target: NoteTarget,
) -> Result<Json<Value>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .update_note(&identity, parse(body, target)?)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}
pub(crate) async fn file(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(file): ApiPath<String>,
    ApiJson(body): ApiJson<NoteBody>,
) -> Result<Json<Value>, ApiError> {
    body.matches_path("file_id", &file)?;
    submit(state, identity, body, NoteTarget::File(file)).await
}
pub(crate) async fn version(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(version): ApiPath<String>,
    ApiJson(body): ApiJson<NoteBody>,
) -> Result<Json<Value>, ApiError> {
    body.matches_path("version_id", &version)?;
    submit(state, identity, body, NoteTarget::Version(version)).await
}
pub(crate) async fn node(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath((file, node)): ApiPath<(String, String)>,
    ApiJson(body): ApiJson<NoteBody>,
) -> Result<Json<Value>, ApiError> {
    body.matches_path("file_id", &file)?;
    body.matches_path("node_id", &node)?;
    submit(state, identity, body, NoteTarget::Node { file, node }).await
}
#[cfg(test)]
#[path = "../../../test/server/unit/http_note.rs"]
mod tests;
