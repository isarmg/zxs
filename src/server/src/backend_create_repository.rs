//! Parsed repository creation input; live placement remains an engine decision.
pub struct CreateRepository {
    pub name: storage_contract::RepositoryName,
    pub primary_slot_id: String,
    pub target_copies: u32,
    pub operation_id: storage_contract::OperationId,
}
