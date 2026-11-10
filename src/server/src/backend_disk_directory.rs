//! Parsed commands over the file manager's displayed disk directory namespace.
#[derive(Clone, Debug)]
pub enum DiskDirectoryAction {
    Create,
    ReviewedCreate(storage_contract::directory_git::DirectoryBranchCreateSource),
    Select,
}
pub struct MutateDiskDirectory {
    pub slot_id: String,
    pub path: Option<name_policy::LookupPath>,
    pub name: storage_contract::BranchName,
    pub operation_id: storage_contract::OperationId,
    pub action: DiskDirectoryAction,
}
