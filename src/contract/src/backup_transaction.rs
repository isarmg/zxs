//! Explicit administrator control of one retained whole-pool backup transaction.
//! Discovery observations and progress are not publication or activation proofs.
use crate::{ErrorCode, OperationId, OperationResult, storage_directories::*};
use data_serialization::{Deserialize, Serialize};

pub const MAX_REQUEST_BYTES: usize = 128 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Target {
    pub selection: CandidateSelection,
    pub disk_id: String,
    pub slot_id: String,
    pub upstream_slot_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct StartRequest {
    pub operation_id: OperationId,
    pub source_slot_id: String,
    #[serialization(with = "crate::u64_string")]
    pub expected_sequence: u64,
    pub targets: Vec<Target>,
    pub confirmation: PreparationConfirmation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum PreparationConfirmation {
    PrepareIndependentCompleteCopies,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum StopConfirmation {
    StopAllOriginalPoolWrites,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum DecisionConfirmation {
    SelectOriginalIrreversibleRegistryDecision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum ActivationConfirmation {
    ActivateOnlyTheOriginalPublishedBackup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum ResumeConfirmation {
    RecoverOnlyTheOriginalRetainedTask,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    ContinuePreparation {},
    ResumeOriginalTask {
        confirmation: ResumeConfirmation,
    },
    FenceSource {
        confirmation: StopConfirmation,
    },
    DecideRegistry {
        confirmation: DecisionConfirmation,
    },
    Activate {
        confirmation: ActivationConfirmation,
    },
}

/// The core's retained input, not caller-supplied replacement paths on retry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct OriginalRequest {
    pub operation_id: OperationId,
    pub source_slot_id: String,
    #[serialization(with = "crate::u64_string")]
    pub expected_sequence: u64,
    pub targets: Vec<OriginalTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct OriginalTarget {
    pub root_path: String,
    pub disk_id: String,
    pub slot_id: String,
    pub upstream_slot_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum Observation {
    Preparing,
    Blocked,
    SourceChanged,
    ServingActive,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Progress {
    pub observation: Observation,
    /// Diagnostic name only. Clients do not derive permission or success from it.
    pub stage: String,
    #[serialization(with = "crate::u64_string")]
    pub completed_files: u64,
    #[serialization(with = "crate::u64_string")]
    pub total_files: u64,
    #[serialization(with = "crate::u64_string")]
    pub completed_bytes: u64,
    #[serialization(with = "crate::u64_string")]
    pub total_bytes: u64,
    pub error_code: Option<ErrorCode>,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Snapshot {
    pub request: OriginalRequest,
    pub progress: Progress,
    /// Present only after current serving admission AND the exact native receipt.
    /// A first Publication, copied bytes or a finished background worker is not enough.
    pub completion: Option<OperationResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Current {
    /// The retained SYSTEM input survives absence of an in-memory worker.
    pub request: Option<OriginalRequest>,
    pub transaction: Option<Snapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandResponse {
    Observed {
        snapshot: Snapshot,
    },
    /// The owned management recovery worker was queued, not a completed backup.
    RecoveryQueued {
        operation_id: OperationId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Context {
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    pub source_slots: Vec<String>,
    pub discovery: DiscoveredDisks,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/backup_transaction.rs"]
mod tests;
