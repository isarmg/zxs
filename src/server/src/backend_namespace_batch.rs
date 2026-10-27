//! Atomic edits of explicit directory branch compositions.
pub struct NamespaceBatch {
    pub slot_id: String,
    pub request: storage_contract::DirectoryNamespaceBatch,
    pub operation_id: storage_contract::OperationId,
}
