//! Transport declarations for one selected container's durable working state.
use data_serialization::{Deserialize, Serialize};
use storage_contract::file_draft::{DraftBase, DraftCommand, DraftLocation, FileWorkingState};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
pub struct ChangeFileWorking {
    pub operation_id: storage_contract::OperationId,
    pub command: DraftCommand,
}

#[derive(Debug, Serialize)]
pub struct FileWorkingSnapshot {
    pub base: DraftBase,
    pub working: FileWorkingState,
}

/// The read job and response share one deadline. This is not durable authority.
pub struct ReadFileDraft {
    pub target: DraftLocation,
    pub deadline: std::time::Instant,
}
