//! A bounded authenticated read of the registered initial snapshot branch.
use crate::http_boundary::{ApiPath, Authenticated};
use crate::{ApiError, AppState};
use http_routing::{Json, extract::State};
use storage_contract::RepositoryId;
use storage_contract::snapshot_worktree::discovery::DirectorySnapshotHead;

pub(crate) async fn head(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(repository): ApiPath<RepositoryId>,
) -> Result<Json<DirectorySnapshotHead>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.directory_snapshot_head(&identity, repository))
        .await
        .map(Json)
}
