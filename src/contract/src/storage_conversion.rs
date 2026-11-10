//! Reviewed, non-destructive ordinary-directory conversion into one new primary.
use crate::{
    OperationId, OperationResult,
    storage_addition::{AdditionFailure, AdditionPreview, AdditionStatus},
    storage_directories::{CandidateSelection, DirectorySource},
};
use data_serialization::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct OrdinaryDiskConversionRequest {
    pub operation_id: OperationId,
    pub source: DirectorySource,
    pub target: CandidateSelection,
    pub slot_id: String,
    pub metadata_member: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct OrdinarySourcePreview {
    pub digest: String,
    pub files: usize,
    pub directories: usize,
    #[serialization(with = "crate::u64_string")]
    pub content_bytes: u64,
    #[serialization(with = "crate::u64_string")]
    pub temporary_bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct OrdinaryDiskConversionPreview {
    pub operation_id: OperationId,
    pub import_operation_id: OperationId,
    pub source: DirectorySource,
    pub source_preview: OrdinarySourcePreview,
    pub target: AdditionPreview,
    pub preview_digest: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct OrdinaryDiskConversionCommit {
    pub preview: OrdinaryDiskConversionPreview,
    pub acknowledge_contents_only: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum OrdinaryDiskConversionPhase {
    Registering,
    Importing,
    Completed,
    RecoveryRequired,
    Stopping,
    Stopped,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct OrdinaryDiskConversionStatus {
    pub preview: OrdinaryDiskConversionPreview,
    pub phase: OrdinaryDiskConversionPhase,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub registration: Option<AdditionStatus>,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub workspace_id: Option<String>,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub root_operation_id: Option<OperationId>,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub result: Option<OperationResult>,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub stop_operation_id: Option<OperationId>,
    /// Exact permanent stop receipt, never a successful content import result.
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub stop_result: Option<OperationResult>,
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub failure: Option<AdditionFailure>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct OrdinaryDiskConversionStatusResponse {
    #[serialization(deserialize_with = "crate::encoding::required_nullable")]
    pub operation: Option<OrdinaryDiskConversionStatus>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct OrdinaryDiskConversionResume {
    pub operation_id: OperationId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct OrdinaryDiskConversionStop {
    pub operation_id: OperationId,
    pub acknowledge_retained_state: bool,
}
