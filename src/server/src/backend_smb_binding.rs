//! Request-local SMB share metadata, distinct from an authenticated binding token.
use data_serialization::Serialize;

#[derive(Debug)]
pub struct SmbBindingStatus {
    pub repository_id: String,
    pub branch_id: String,
    pub enabled: bool,
    pub history_generation: u64,
}
pub struct UpdateSmbBinding<'a> {
    pub repository: &'a str,
    pub branch: &'a str,
    pub enabled: bool,
    pub expected_history_generation: u64,
    pub operation: storage_contract::OperationId,
}
#[derive(Serialize)]
pub(crate) struct SmbBindingResponse {
    repository_id: String,
    branch_id: String,
    enabled: bool,
    #[serialization(with = "storage_contract::u64_string")]
    history_generation: u64,
}
impl From<SmbBindingStatus> for SmbBindingResponse {
    fn from(status: SmbBindingStatus) -> Self {
        let SmbBindingStatus {
            repository_id,
            branch_id,
            enabled,
            history_generation,
        } = status;
        Self {
            repository_id,
            branch_id,
            enabled,
            history_generation,
        }
    }
}
