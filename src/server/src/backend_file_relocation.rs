pub struct MoveDiskFile {
    pub slot_id: name_policy::SlotId,
    pub relocation: storage_contract::file_relocation::FileRelocation,
    pub operation_id: storage_contract::OperationId,
}
