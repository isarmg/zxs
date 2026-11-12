//! Decode the public directory-selection command once at HTTP ingress.
use crate::{
    ApiError, AppState, SelectDefaultDirectoryBranch, auth::Identity, http_boundary::ApiPath,
};
use http_routing::{
    Extension, Json,
    extract::{Request, State},
};
use json_codec::{Map, Value};

fn parse(mut body: Map<String, Value>) -> Result<SelectDefaultDirectoryBranch, ApiError> {
    if let Some(key) = body.keys().find(|key| {
        ![
            "repository_id",
            "directory_branch_id",
            "expected_tree_id",
            "expected_head",
            "operation_id",
        ]
        .contains(&key.as_str())
    }) {
        return Err(ApiError::invalid(format!("unknown field {key}")));
    }
    let repository_id = match body.remove("repository_id") {
        Some(Value::String(id)) => id,
        _ => unreachable!("invariant: with_id binds the repository path to an object body"),
    };
    let mut field = |key: &str| match body.remove(key) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value),
        _ => Err(ApiError::invalid(format!("missing string field {key}"))),
    };
    let directory_branch_id = field("directory_branch_id")?;
    let operation_id = field("operation_id")?
        .parse::<storage_contract::OperationId>()
        .map_err(contract_input)?;
    let expected_tree_id = field("expected_tree_id")?;
    let expected_head = field("expected_head")?;
    if directory_branch_id.len() > 256 || expected_tree_id.len() > 256 || expected_head.len() > 256
    {
        return Err(ApiError::invalid(
            "default selection identities exceed 256 bytes",
        ));
    }
    Ok(SelectDefaultDirectoryBranch {
        repository_id,
        directory_branch_id,
        expected_tree_id,
        expected_head,
        operation_id,
    })
}

pub(crate) async fn select(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(id): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    let value = crate::with_id(crate::json_body(request).await?, "repository_id", id)?;
    let Value::Object(body) = value else {
        unreachable!("invariant: with_id binds an object body")
    };
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            let command = parse(body)?;
            backend
                .select_default_directory_branch(&identity, command)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}

fn contract_input(source: storage_contract::Error) -> ApiError {
    ApiError::new(
        http::StatusCode::BAD_REQUEST,
        source.code(),
        source.message().into_owned(),
    )
    .with_source(source)
}

#[cfg(test)]
#[path = "../../../test/server/unit/http_default_directory.rs"]
mod tests;
