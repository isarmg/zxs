//! A parsed existing-directory reference; `None` identifies the repository root.
pub struct InitializeDirectoryScope {
    pub repository_id: String,
    pub path: Option<name_policy::LookupPath>,
    pub operation_id: storage_contract::OperationId,
}
