//! Decode explicit historical sources and a new result branch at the HTTP boundary.
use crate::http_file_metadata::{location, operation, string};
use crate::{ApiError, AppState, MergeFileHistory, auth::Identity, http_boundary::ApiPath};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};
fn parse(mut body: Map<String, Value>) -> Result<MergeFileHistory, ApiError> {
    if let Some(key) = body.keys().find(|key| {
        ![
            "file_id",
            "inputs",
            "chosen_node_id",
            "name",
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
    let raw_name = string(body.remove("name"), "name")?;
    let name = storage_contract::BranchName::parse(&raw_name).map_err(|source| {
        ApiError::new(
            http::StatusCode::BAD_REQUEST,
            source.code(),
            source.message().into_owned(),
        )
        .with_source(source)
    })?;
    let confirmed = matches!(body.remove("confirmed"), Some(Value::Bool(true)));
    let location = location(&mut body)?;
    let operation_id = operation(body.remove("operation_id"))?;
    Ok(MergeFileHistory {
        file_id,
        inputs,
        chosen_node_id,
        name,
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
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend
                .merge_file_history(&identity, parse(body)?)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_file_history_merge.rs"]
mod tests;
