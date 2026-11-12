//! One current-context CAS replacement of the file container's shared tags.
pub struct ReplaceContainerTags {
    pub command: storage_contract::file_container_tags::ReplaceCommand,
    pub operation_id: storage_contract::OperationId,
}
