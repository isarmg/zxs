//! Strict typed read routes; lookup never resubmits or finishes a mutation.
use crate::http_boundary::{ApiPath, Authenticated};
use crate::{ApiError, AppState};
use http_routing::{Json, extract::State};
use storage_contract::snapshot_worktree::outcome::{
    WorktreeCommitReceipt, WorktreeCreationReceipt,
};
use storage_contract::{OperationId, RepositoryId};

pub(crate) async fn creation(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath((repository, operation)): ApiPath<(RepositoryId, OperationId)>,
) -> Result<Json<WorktreeCreationReceipt>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend.directory_worktree_creation_receipt(&identity, repository, operation)
        })
        .await
        .map(Json)
}

pub(crate) async fn commit(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath((repository, operation)): ApiPath<(RepositoryId, OperationId)>,
) -> Result<Json<WorktreeCommitReceipt>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| {
            backend.directory_worktree_commit_receipt(&identity, repository, operation)
        })
        .await
        .map(Json)
}
