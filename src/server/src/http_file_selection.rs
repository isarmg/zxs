//! Parse file selection context once without turning a missing context into a path.
use crate::{
    ApiError, AppState, FileSelectionLocation, SelectFileBranch, auth::Identity,
    http_boundary::ApiPath,
};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};
fn string(value: Option<Value>, key: &str) -> Result<String, ApiError> {
    match value {
        Some(Value::String(value)) if !value.is_empty() => Ok(value),
        _ => Err(ApiError::invalid(format!("missing string field {key}"))),
    }
}
fn parse(mut body: Map<String, Value>) -> Result<SelectFileBranch, ApiError> {
    if let Some(key) = body.keys().find(|key| {
        ![
            "file_id",
            "branch_id",
            "operation_id",
            "repository_id",
            "path",
        ]
        .contains(&key.as_str())
    }) {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let Some(Value::String(file_id)) = body.remove("file_id") else {
        unreachable!("invariant: with_id binds the file path")
    };
    let location = match (body.remove("repository_id"), body.remove("path")) {
        (None, None) => None,
        (Some(repository), Some(path)) => Some((
            string(Some(repository), "repository_id")?,
            string(Some(path), "path")?,
        )),
        (Some(_), None) | (None, Some(_)) => {
            return Err(ApiError::invalid(
                "repository_id and path must be supplied together",
            ));
        }
    };
    let branch_id = string(body.remove("branch_id"), "branch_id")?;
    let operation_id = string(body.remove("operation_id"), "operation_id")?
        .parse()
        .map_err(ApiError::invalid_source)?;
    let location = location
        .map(|(repository_id, path)| {
            let path = name_policy::PortablePath::parse(&path).map_err(|source| {
                let source = storage_contract::Error::from(source);
                ApiError::new(
                    http::StatusCode::BAD_REQUEST,
                    source.code(),
                    source.message().into_owned(),
                )
                .with_source(source)
            })?;
            Ok::<_, ApiError>(FileSelectionLocation {
                repository_id,
                path,
            })
        })
        .transpose()?;
    Ok(SelectFileBranch {
        file_id,
        branch_id,
        location,
        operation_id,
    })
}
pub(crate) async fn select(
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
                .select_file_branch(&identity, parse(body)?)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}
#[cfg(test)]
#[path = "../../../test/server/unit/http_file_selection.rs"]
mod tests;
