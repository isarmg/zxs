//! Complete bounded instance discovery; selection remains an exact status read.
use crate::http_boundary::{ApiPath, Authenticated};
use crate::{ApiError, AppState};
use http_routing::{Json, extract::State};
use storage_contract::RepositoryId;
use storage_contract::snapshot_worktree::instances::DirectoryWorktrees;

pub(crate) async fn read(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(repository): ApiPath<RepositoryId>,
) -> Result<Json<DirectoryWorktrees>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.directory_worktree_instances(&identity, repository))
        .await
        .map(Json)
}
