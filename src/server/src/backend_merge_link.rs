//! Parsed relation registration; history ownership remains live engine admission.
pub struct CreateMergeLink {
    pub history_tree_id: String,
    pub source_branch_id: String,
    pub source_commit_id: String,
    pub target_branch_id: String,
    pub target_commit_id: String,
    pub operation_id: storage_contract::OperationId,
}
