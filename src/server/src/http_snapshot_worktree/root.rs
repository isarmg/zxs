//! The empty-disk entry point uses the same typed HTTP admission as snapshots.
use crate::http_boundary::{ApiJson, ApiPath, Authenticated, JsonBody};
use crate::{ApiError, AppState};
use http_routing::{Json, extract::State};
use name_policy::SlotId;
use storage_contract::snapshot_worktree::root::{RegisterSnapshotRoot, RegisteredSnapshotRoot};

pub(crate) struct RegisterBody(RegisterSnapshotRoot);
impl JsonBody for RegisterBody {
    fn from_json(value: json_codec::Value) -> Result<Self, ApiError> {
        if !value.is_object() {
            return Err(ApiError::invalid(
                "root registration must be a named object",
            ));
        }
        json_codec::from_value(value)
            .map(Self)
            .map_err(ApiError::invalid_source)
    }
}

pub(crate) async fn register(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(slot): ApiPath<SlotId>,
    ApiJson(request): ApiJson<RegisterBody>,
) -> Result<Json<RegisteredSnapshotRoot>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.register_snapshot_root(&identity, slot, request.0))
        .await
        .map(Json)
}
