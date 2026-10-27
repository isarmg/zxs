//! A repository-scoped creation command decoded at transport ingress.
pub struct CreateBranch {
    pub repository_id: String,
    pub source_commit_id: String,
    pub name: storage_contract::BranchName,
    pub operation_id: storage_contract::OperationId,
}
