//! A bounded current-prefix history read. Partial cursors never become a new page.
use crate::http_boundary::{ApiPath, ApiQuery, Authenticated};
use crate::{ApiError, AppState};
use data_serialization::Deserialize;
use http_routing::{Json, extract::State};
use storage_contract::snapshot_worktree::SnapshotHandle;
use storage_contract::snapshot_worktree::history::{
    DirectorySnapshotHistory, SnapshotHistoryCursor,
};
use storage_contract::working_tree::{SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId};
use storage_contract::{OperationId, RepositoryId, Sequence};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct HistoryQuery {
    sequence: Option<Sequence>,
    publication: Option<OperationId>,
    repository_id: Option<SnapshotRepositoryId>,
    branch_id: Option<SnapshotBranchId>,
    node_id: Option<SnapshotNodeId>,
}

impl HistoryQuery {
    fn cursor(self) -> Result<Option<SnapshotHistoryCursor>, ApiError> {
        match (
            self.sequence,
            self.publication,
            self.repository_id,
            self.branch_id,
            self.node_id,
        ) {
            (None, None, None, None, None) => Ok(None),
            (
                Some(sequence),
                Some(publication),
                Some(repository_id),
                Some(branch_id),
                Some(node_id),
            ) => Ok(Some(SnapshotHistoryCursor {
                sequence,
                after: SnapshotHandle {
                    publication,
                    repository_id,
                    branch_id,
                    node_id,
                },
            })),
            _ => Err(ApiError::invalid(
                "a snapshot history cursor requires all five fields",
            )),
        }
    }
}

pub(crate) async fn read(
    State(state): State<AppState>,
    Authenticated(identity): Authenticated,
    ApiPath(repository): ApiPath<RepositoryId>,
    ApiQuery(query): ApiQuery<HistoryQuery>,
) -> Result<Json<DirectorySnapshotHistory>, ApiError> {
    let cursor = query.cursor()?;
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.directory_snapshot_history(&identity, repository, cursor))
        .await
        .map(Json)
}
