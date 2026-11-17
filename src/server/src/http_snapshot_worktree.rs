//! Strict management requests over the existing authentication, CSRF, JSON
//! byte/duplicate-key boundary and bounded blocking backend job owner.
pub(crate) mod drafts;

use crate::{ApiError, AppState};
use crate::http_boundary::{ApiJson, ApiPath, ApiQuery, Authenticated, JsonBody};
use data_serialization::Deserialize;
use http_routing::{Json, extract::State};
use storage_contract::snapshot_worktree::{
    CreateWorktree, InitializeSnapshot, SnapshotHandle, SnapshotRootContext,
    WorktreeHandle, WorktreeId, WorktreeStatus,
};

pub(crate) struct InitializeBody(InitializeSnapshot);
impl JsonBody for InitializeBody {
    fn from_json(value: json_codec::Value) -> Result<Self, ApiError> {
        if !value.get("host").is_some_and(json_codec::Value::is_object)
            || !value.get("source").is_some_and(json_codec::Value::is_object)
        {
            return Err(ApiError::invalid("host and source must be named objects"));
        }
        json_codec::from_value(value).map(Self).map_err(ApiError::invalid_source)
    }
}

pub(crate) struct CreateBody(CreateWorktree);
impl JsonBody for CreateBody {
    fn from_json(value: json_codec::Value) -> Result<Self, ApiError> {
        if !value.get("source").is_some_and(json_codec::Value::is_object) {
            return Err(ApiError::invalid("source must be a named object"));
        }
        json_codec::from_value(value).map(Self).map_err(ApiError::invalid_source)
    }
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct StatusQuery {
    publication: storage_contract::OperationId,
}

pub(crate) async fn context(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiPath(repository): ApiPath<storage_contract::RepositoryId>,
) -> Result<Json<SnapshotRootContext>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.snapshot_root_context(&identity, repository))
        .await.map(Json)
}

pub(crate) async fn initialize(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiJson(request): ApiJson<InitializeBody>,
) -> Result<Json<SnapshotHandle>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.initialize_directory_snapshot(&identity, request.0))
        .await.map(Json)
}

pub(crate) async fn create(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiJson(request): ApiJson<CreateBody>,
) -> Result<Json<WorktreeHandle>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.create_directory_worktree(&identity, request.0))
        .await.map(Json)
}

pub(crate) async fn status(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiPath(worktree_id): ApiPath<WorktreeId>, ApiQuery(query): ApiQuery<StatusQuery>,
) -> Result<Json<WorktreeStatus>, ApiError> {
    let target = WorktreeHandle { publication: query.publication, worktree_id };
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.directory_worktree_status(&identity, target))
        .await.map(Json)
}
