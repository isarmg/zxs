//! Parsed commands over the file manager's displayed disk directory namespace.
#[derive(Clone, Copy, Debug)]
pub enum DiskDirectoryAction {
    Create,
    Select,
}
pub struct MutateDiskDirectory {
    pub slot_id: String,
    pub path: Option<name_policy::LookupPath>,
    pub name: storage_contract::BranchName,
    pub operation_id: storage_contract::OperationId,
    pub action: DiskDirectoryAction,
}
