//! Non-persisted publication facts for result reconciliation.
use crate::{CommitState, OperationId, OperationResult, ProtectionAtPublish};

/// A receipt projection, not a current content-health observation or authorization.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationOutcome {
    pub operation_id: OperationId,
    pub kind: String,
    pub commit_id: Option<String>,
    pub history_tree_id: Option<String>,
    pub branch_id: Option<String>,
    pub commit_state: CommitState,
    pub protection_at_publish: ProtectionAtPublish,
    pub request_digest: String,
    pub sequence: u64,
    pub result: json_codec::Value,
}

impl From<OperationResult> for OperationOutcome {
    fn from(result: OperationResult) -> Self {
        let OperationResult {
            operation_id,
            kind,
            commit_id,
            history_tree_id,
            branch_id,
            commit_state,
            protection_at_publish,
            current_health: _,
            request_digest,
            sequence,
            result,
        } = result;
        Self {
            operation_id,
            kind,
            commit_id,
            history_tree_id,
            branch_id,
            commit_state,
            protection_at_publish,
            request_digest,
            sequence,
            result,
        }
    }
}
