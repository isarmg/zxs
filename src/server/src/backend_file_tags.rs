//! One atomic update of explicitly reviewed file-node tag targets.
pub struct UpdateFileNodeTags {
    pub file_id: String,
    pub targets: Vec<storage_contract::FileTagTarget>,
    pub add: Vec<String>,
    pub remove: Vec<String>,
    pub location: Option<storage_contract::FileLocation>,
    pub operation_id: storage_contract::OperationId,
}
