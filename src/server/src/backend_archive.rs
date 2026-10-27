//! Archive keeps its established raw receipt, distinct from operation observation envelopes.
use data_serialization::Serialize;
use storage_contract::{
    CommitState, CurrentHealth, OperationId, OperationResult, ProtectionAtPublish,
};

pub struct ArchiveBranch {
    pub branch_id: String,
    pub operation_id: OperationId,
}
#[derive(Serialize)]
pub(crate) struct ArchiveResponse {
    operation_id: OperationId,
    kind: String,
    commit_id: Option<String>,
    history_tree_id: Option<String>,
    branch_id: Option<String>,
    commit_state: CommitState,
    protection_at_publish: ProtectionAtPublish,
    current_health: CurrentHealth,
    request_digest: String,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    result: json_codec::Value,
}
impl From<OperationResult> for ArchiveResponse {
    fn from(receipt: OperationResult) -> Self {
        let OperationResult {
            operation_id,
            kind,
            commit_id,
            history_tree_id,
            branch_id,
            commit_state,
            protection_at_publish,
            current_health,
            request_digest,
            sequence,
            result,
        } = receipt;
        Self {
            operation_id,
            kind,
            commit_id,
            history_tree_id,
            branch_id,
            commit_state,
            protection_at_publish,
            current_health,
            request_digest,
            sequence,
            result,
        }
    }
}
#[cfg(test)]
#[path = "../../../test/server/unit/backend_archive.rs"]
mod tests;
