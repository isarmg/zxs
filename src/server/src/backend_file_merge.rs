//! Explicit adoption of a chosen node and surviving file branch.
pub struct MergeFileBranches {
    pub file_id: String,
    pub inputs: Vec<storage_contract::FileMergeInput>,
    pub chosen_node_id: String,
    pub surviving_branch_id: String,
    pub confirmed: bool,
    pub location: Option<storage_contract::FileLocation>,
    pub operation_id: storage_contract::OperationId,
}

/// Historical sources are read without advancing or freezing their branches.
pub struct MergeFileHistory {
    pub file_id: String,
    pub inputs: Vec<storage_contract::FileHistoryMergeInput>,
    pub chosen_node_id: String,
    pub name: storage_contract::BranchName,
    pub confirmed: bool,
    pub location: Option<storage_contract::FileLocation>,
    pub operation_id: storage_contract::OperationId,
}
