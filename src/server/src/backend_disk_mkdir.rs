//! Create a directory in a controlled displayed disk namespace.
pub struct CreateDiskDirectory {
    pub slot_id: String,
    pub path: name_policy::PortablePath,
    pub operation_id: storage_contract::OperationId,
}
