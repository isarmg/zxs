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
