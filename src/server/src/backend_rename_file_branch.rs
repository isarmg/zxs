//! A parsed public file-branch label mutation, independent of immutable content.
pub struct RenameFileBranch {
    pub file_id: String,
    pub branch_id: String,
    pub name: storage_contract::BranchName,
    pub operation_id: storage_contract::OperationId,
}
