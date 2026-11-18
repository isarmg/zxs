//! Only the exact registered handle and original operation, never a mount path.
use crate::http_boundary::{ApiJson, ApiPath, Authenticated, JsonBody};
use crate::{ApiError, AppState};
use http_routing::{Json, extract::State};
use storage_contract::{OperationId, RepositoryId};
use storage_contract::snapshot_worktree::{WorktreeId, mount_change::{MountChange, MountChangeReceipt}};

pub(crate) struct Body(MountChange);
impl JsonBody for Body {
    fn from_json(value: json_codec::Value) -> Result<Self, ApiError> {
        if !["target", "expected", "action"].into_iter().all(|k| value.get(k).is_some_and(json_codec::Value::is_object)) {
            return Err(ApiError::invalid("target, expected and action must be named objects"));
        }
        json_codec::from_value(value).map(Self).map_err(ApiError::invalid_source)
    }
}

pub(crate) async fn change(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiPath(worktree): ApiPath<WorktreeId>, ApiJson(body): ApiJson<Body>,
) -> Result<Json<MountChangeReceipt>, ApiError> {
    if body.0.target.worktree_id != worktree { return Err(ApiError::invalid("mount target differs from route")); }
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.change_directory_worktree_mount(&identity, body.0)).await.map(Json)
}

pub(crate) async fn receipt(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiPath((repository, operation)): ApiPath<(RepositoryId, OperationId)>,
) -> Result<Json<MountChangeReceipt>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.directory_worktree_mount_receipt(&identity, repository, operation)).await.map(Json)
}
