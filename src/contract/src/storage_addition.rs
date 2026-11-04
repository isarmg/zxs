//! Existing-pool directory addition. These receipts are distinct from first setup.
use crate::{ErrorCode, OperationId, OperationResult, storage_directories::CandidateSelection};
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

/// The supplied hash binds a preview, but is never an authentication token.
/// Terminal authority is loaded and authenticated by the server itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdditionAuthority {
    ActiveMembership,
    TerminalManagement {
        terminal_authority_hash: String,
        terminal_operation_id: OperationId,
    },
}
impl AdditionAuthority {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::TerminalManagement { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct AdditionPreview {
    pub authority: AdditionAuthority,
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
    /// Explicitly choosing Add after a failed cancellation does not override
    /// any immutable Abort vote. The engine decides which outcome is possible.
    pub continuation: AdditionContinuation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct AdditionCancel {
    pub operation_id: OperationId,
    pub preview_digest: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum AdditionContinuation {
    Add,
    Cancel,
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
    CertifyingAbort,
    AbortCertified,
    Cancelling,
    Completed,
    Cancelled,
}

impl AdditionPhase {
    pub fn is_metadata_resume(self) -> bool {
        matches!(
            self,
            Self::Prepared
                | Self::Enrolling
                | Self::JointDecision
                | Self::Activating
                | Self::Completed
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum AdditionState {
    Adding,
    Cancelling,
    RecoveryRequired,
    Completed,
    Cancelled,
}
impl AdditionState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct AdditionFailure {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct AdditionStatus {
    pub operation_id: OperationId,
    pub state: AdditionState,
    pub phase: AdditionPhase,
    pub continuation: AdditionContinuation,
    /// Exact permanent engine result, present only after actual publication.
    pub result: Option<OperationResult>,
    pub preview: AdditionPreview,
    pub failure: Option<AdditionFailure>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct AdditionStatusResponse {
    pub operation: Option<AdditionStatus>,
}
