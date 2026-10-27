//! A public file branch created from a frozen version and optional explicit node.
pub struct CreateFileBranch {
    pub file_id: String,
    pub source_version_id: String,
    pub source_node_id: Option<String>,
    pub name: storage_contract::BranchName,
    pub operation_id: storage_contract::OperationId,
}
