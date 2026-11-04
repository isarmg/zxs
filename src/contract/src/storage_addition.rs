//! Existing-pool directory addition. These receipts are distinct from first setup.
use crate::{ErrorCode, OperationId, storage_directories::CandidateSelection};
use data_serialization::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryAddition {
    pub selection: CandidateSelection,
    pub slot_id: String,
    pub level: u32,
    pub upstream_slot_id: Option<String>,
    pub metadata_member: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct AdditionRequest {
    pub operation_id: OperationId,
    pub additions: Vec<DirectoryAddition>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct AdditionPreview {
    pub request: AdditionRequest,
    pub pool_id: String,
    pub configuration_digest: String,
    pub preview_digest: String,
    pub original_metadata_members: usize,
    pub target_metadata_members: usize,
    /// Directory count does not prove independent physical failure domains.
    pub added_directories: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct AdditionCommit {
    pub preview: AdditionPreview,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct AdditionResume {
    pub operation_id: OperationId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum AdditionPhase {
    Prepared,
    Fencing,
    Archiving,
    Enrolling,
    JointDecision,
    Activating,
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum AdditionState {
    Adding,
    RecoveryRequired,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct AdditionFailure {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct AdditionStatus {
    pub operation_id: OperationId,
    pub state: AdditionState,
    pub phase: AdditionPhase,
    pub preview: AdditionPreview,
    pub failure: Option<AdditionFailure>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct AdditionStatusResponse {
    pub operation: Option<AdditionStatus>,
}
