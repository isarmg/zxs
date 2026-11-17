//! Authenticated explicit selection over the shared strict JSON/CSRF boundary.
//! Responses never serialize native directory receipts or physical identities.
use crate::http_boundary::{ApiJson, ApiPath, ApiQuery, Authenticated, JsonBody};
use crate::{ApiError, AppState};
use data_serialization::Deserialize;
use http_routing::{Json, extract::State};
use storage_contract::snapshot_worktree::draft_commit::{
    CommitDirectoryDrafts, DirectoryDraftLookup, DirectoryDraftReceipt,
};
use storage_contract::snapshot_worktree::SnapshotHandle;
use storage_contract::working_tree::{SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId};
use storage_contract::OperationId;

pub(crate) struct CommitBody(CommitDirectoryDrafts);
impl JsonBody for CommitBody {
    fn from_json(value: json_codec::Value) -> Result<Self, ApiError> {
        if !value.get("source").is_some_and(json_codec::Value::is_object) {
            return Err(ApiError::invalid("source must be a named object"));
        }
        let selected = value.get("selected").and_then(json_codec::Value::as_array)
            .ok_or_else(|| ApiError::invalid("selected must be an array"))?;
        for item in selected {
            let expected = item.get("expected");
            if !item.get("target").is_some_and(json_codec::Value::is_object)
                || !expected.is_some_and(json_codec::Value::is_object)
                || !(expected.and_then(|value| value.get("content"))
                    .is_some_and(json_codec::Value::is_object)
                    || (expected.and_then(|value| value.get("base"))
                        .is_some_and(json_codec::Value::is_object)
                        && expected.and_then(|value| value.get("payload"))
                            .is_some_and(json_codec::Value::is_object)))
            {
                return Err(ApiError::invalid(
                    "selected target and its Born or Unborn expectation must be objects",
                ));
            }
        }
        json_codec::from_value(value).map(Self).map_err(ApiError::invalid_source)
    }
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct ReceiptQuery {
    source_publication: OperationId,
    repository_id: SnapshotRepositoryId,
    branch_id: SnapshotBranchId,
    node_id: SnapshotNodeId,
}

pub(crate) async fn commit(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiJson(request): ApiJson<CommitBody>,
) -> Result<Json<DirectoryDraftReceipt>, ApiError> {
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.commit_directory_drafts(&identity, request.0))
        .await.map(Json)
}

pub(crate) async fn receipt(
    State(state): State<AppState>, Authenticated(identity): Authenticated,
    ApiPath(operation_id): ApiPath<OperationId>, ApiQuery(query): ApiQuery<ReceiptQuery>,
) -> Result<Json<DirectoryDraftReceipt>, ApiError> {
    let lookup = DirectoryDraftLookup {
        source: SnapshotHandle {
            publication: query.source_publication, repository_id: query.repository_id,
            branch_id: query.branch_id, node_id: query.node_id,
        },
        operation_id,
    };
    crate::http_job::HttpBackendJob::admit(state.backend, state.auth, state.backend_slots)?
        .run(move |backend| backend.directory_draft_receipt(&identity, lookup))
        .await.map(Json)
}
