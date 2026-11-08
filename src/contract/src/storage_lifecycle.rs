//! Explicit lifecycle actions on current ZXS directory registrations.
use crate::{OperationId, OperationResult, storage_addition::AdditionFailure};
use data_serialization::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum ExitAction {
    PlannedOffline,
    Deregister,
}
impl ExitAction {
    pub fn result_kind(self) -> &'static str {
        match self {
            Self::PlannedOffline => "disk-planned-offline",
            Self::Deregister => "disk-deregister",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ExitRequest {
    pub operation_id: OperationId,
    pub slot_id: String,
    pub expected_disk_id: String,
    pub action: ExitAction,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ExitTarget {
    pub pool_id: String,
    pub disk_id: String,
    pub slot_id: String,
    #[serialization(with = "crate::u64_string")]
    pub binding_generation: u64,
    /// Display only. Clients cannot supply an operating-system device action.
    pub root_path: String,
}
/// The final voter retires into authenticated management, not a zero-member pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum ExitAuthority {
    ActiveMembership,
    TerminalManagement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum TerminalRegistrations {
    RetainedRegistrations,
    EmptyRegistry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum TerminalRetirementPhase {
    IntentStored,
    DecisionStored,
}
/// Unfinished native selection. Neither phase certifies removal or target I/O closure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TerminalRetirementPending {
    pub operation_id: OperationId,
    pub principal: String,
    pub pool_id: String,
    pub disk_id: String,
    pub slot_id: String,
    #[serialization(with = "crate::u64_string")]
    pub binding_generation: u64,
    pub intent_digest: String,
    pub phase: TerminalRetirementPhase,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TerminalStorageStatus {
    pub pool_id: String,
    pub operation_id: OperationId,
    pub authority_hash: String,
    pub registrations: TerminalRegistrations,
    pub registered_directories: usize,
    /// Historical authenticated denominator, never recalculated from survivors.
    pub original_metadata_members: usize,
    pub original_metadata_quorum: usize,
    #[serialization(deserialize_with = "required_retirement")]
    pub pending_retirement: Option<TerminalRetirementPending>,
    #[serialization(deserialize_with = "required_completed_retirement")]
    pub completed_retirement: Option<OperationId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ExitPreview {
    pub request: ExitRequest,
    pub target: ExitTarget,
    pub authority: ExitAuthority,
    pub configuration_digest: String,
    pub preview_digest: String,
    #[serialization(with = "crate::u64_string")]
    pub source_sequence: u64,
    #[serialization(with = "crate::u64_string")]
    pub topology_generation: u64,
    pub snapshot_hash: String,
    pub original_metadata_members: usize,
    pub target_metadata_members: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ExitCommit {
    pub preview: ExitPreview,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ExitResume {
    pub operation_id: OperationId,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum ExitPhase {
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
pub enum ExitState {
    Running,
    RecoveryRequired,
    Completed,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ExitStatus {
    pub operation_id: OperationId,
    pub state: ExitState,
    pub phase: ExitPhase,
    pub preview: ExitPreview,
    pub failure: Option<AdditionFailure>,
    /// Success requires the exact published lifecycle result and admitted D3 view.
    pub result: Option<OperationResult>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ExitStatusResponse {
    pub operation: Option<ExitStatus>,
}

fn required_retirement<'de, D>(
    deserializer: D,
) -> Result<Option<TerminalRetirementPending>, D::Error>
where
    D: data_serialization::Deserializer<'de>,
{
    Option::deserialize(deserializer)
}

/// One no-voter registration successor, not the final-voter handoff or an active
/// namespace publication. The digest binds the complete native source and actor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TerminalRetirementPreview {
    pub request: ExitRequest,
    pub target: ExitTarget,
    pub principal: String,
    pub intent_digest: String,
    pub source_authority_hash: String,
    pub source_registry_hash: String,
    #[serialization(with = "crate::u64_string")]
    pub source_registry_generation: u64,
    #[serialization(with = "crate::u64_string")]
    pub source_sequence: u64,
    #[serialization(with = "crate::u64_string")]
    pub source_topology_generation: u64,
}

fn required_completed_retirement<'de, D>(deserializer: D) -> Result<Option<OperationId>, D::Error>
where
    D: data_serialization::Deserializer<'de>,
{
    Option::deserialize(deserializer)
}
