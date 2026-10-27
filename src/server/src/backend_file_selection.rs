//! Select a public file branch, optionally at one explicit logical path.
pub struct FileSelectionLocation {
    pub repository_id: String,
    pub path: name_policy::PortablePath,
}
pub struct SelectFileBranch {
    pub file_id: String,
    pub branch_id: String,
    pub location: Option<FileSelectionLocation>,
    pub operation_id: storage_contract::OperationId,
}
