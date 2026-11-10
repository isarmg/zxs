//! Explicit per-directory repository commands. Reads never initialize or stage.
use crate::{
    ApiError, AppState, Backend,
    auth::Identity,
    http_boundary::{ApiPath, ApiQuery},
};
use data_serialization::Deserialize;
use http_routing::{
    Extension, Json,
    extract::{Request, State},
    response::Response,
};
use json_codec::Value;
use storage_contract::directory_git::{
    DirectoryGitInitializeBody, DirectoryGitStageChildBody, DirectoryGitStageOwnedBody,
};

// Status is an exact snapshot, not a silently truncated child list. Retain
// backend admission until these bounded response bytes have been consumed.
const MAX_DIRECTORY_GIT_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct StatusQuery {
    path: String,
}

fn repository_id(value: &str) -> Result<(), ApiError> {
    storage_contract::validate_id(value, "repo").map_err(ApiError::invalid_source)
}

fn directory_id(value: &str, repository: &str) -> Result<(), ApiError> {
    if value == repository {
        Ok(())
    } else {
        storage_contract::validate_id(value, "entry").map_err(ApiError::invalid_source)
    }
}

fn path(value: &str) -> Result<(), ApiError> {
    if !value.is_empty() {
        name_policy::PortablePath::parse(value).map_err(ApiError::invalid_source)?;
    }
    Ok(())
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(repository): ApiPath<String>,
    ApiQuery(query): ApiQuery<StatusQuery>,
) -> Result<Response, ApiError> {
    repository_id(&repository)?;
    if !query.path.is_empty() {
        name_policy::LookupPath::parse(&query.path).map_err(ApiError::invalid_source)?;
    }
    crate::bounded_history_http::read(state, move |backend, cancellation| {
        let status =
            backend.directory_git_status(&identity, &repository, &query.path, cancellation)?;
        crate::history_window_encode::encode_bounded(&status, MAX_DIRECTORY_GIT_RESPONSE_BYTES)
    })
    .await
}

async fn mutate(
    state: AppState,
    work: impl FnOnce(&dyn Backend) -> Result<storage_contract::OperationResult, ApiError>
    + Send
    + 'static,
) -> Result<Json<Value>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            work(backend).and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })
        .await
        .map(Json)
}

pub(crate) async fn initialize(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(repository): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    repository_id(&repository)?;
    let body: DirectoryGitInitializeBody = json_codec::from_value(crate::json_body(request).await?)
        .map_err(ApiError::invalid_source)?;
    path(&body.path)?;
    storage_contract::validate_id(&body.expected_namespace_head, "commit")
        .map_err(ApiError::invalid_source)?;
    mutate(state, move |backend| {
        backend.initialize_directory_git(&identity, &repository, body)
    })
    .await
}

pub(crate) async fn stage_child(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(repository): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    repository_id(&repository)?;
    let body: DirectoryGitStageChildBody = json_codec::from_value(crate::json_body(request).await?)
        .map_err(ApiError::invalid_source)?;
    directory_id(&body.directory_id, &repository)?;
    storage_contract::validate_id(&body.directory_branch_id, "dbr")
        .map_err(ApiError::invalid_source)?;
    storage_contract::validate_id(&body.child_directory_id, "entry")
        .map_err(ApiError::invalid_source)?;
    storage_contract::validate_id(&body.expected_child_head_tree_id, "dtree")
        .map_err(ApiError::invalid_source)?;
    mutate(state, move |backend| {
        backend.stage_directory_git_child(&identity, &repository, body)
    })
    .await
}

pub(crate) async fn stage_owned(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    ApiPath(repository): ApiPath<String>,
    request: Request,
) -> Result<Json<Value>, ApiError> {
    repository_id(&repository)?;
    let body: DirectoryGitStageOwnedBody = json_codec::from_value(crate::json_body(request).await?)
        .map_err(ApiError::invalid_source)?;
    directory_id(&body.directory_id, &repository)?;
    storage_contract::validate_id(&body.directory_branch_id, "dbr")
        .map_err(ApiError::invalid_source)?;
    storage_contract::validate_id(&body.expected_tree_id, "dtree")
        .map_err(ApiError::invalid_source)?;
    mutate(state, move |backend| {
        backend.stage_directory_git_owned(&identity, &repository, body)
    })
    .await
}
