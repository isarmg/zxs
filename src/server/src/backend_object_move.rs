//! One captured current-object move in the displayed source-disk namespace.
pub struct MoveDiskObject {
    pub source_slot_id: String,
    pub movement: storage_contract::file_relocation::ObjectMoveRequest,
    pub operation_id: storage_contract::OperationId,
}
