//! Explicit readmission of one existing Disabled registration, under its exact
//! active membership or authenticated terminal management authority.
use crate::{ErrorCode, OperationId, OperationResult};
use data_serialization::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ReadmissionRequest {
    pub operation_id: OperationId,
    pub slot_id: String,
    pub expected_disk_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ReadmissionTarget {
    pub pool_id: String,
    pub disk_id: String,
    pub slot_id: String,
    #[serialization(with = "crate::u64_string")]
    pub binding_generation: u64,
    /// Existing server-owned path, not a client-selected replacement directory.
    pub root_path: String,
}

/// Compare-and-swap context, never an authentication token. The server reloads
/// the matching authenticated authority before admitting an operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReadmissionAuthority {
    ActiveMembership {
        membership_authority_hash: String,
    },
    TerminalManagement {
        terminal_authority_hash: String,
        terminal_operation_id: OperationId,
    },
}
impl ReadmissionAuthority {
    /// Whether the request restores the first voter from a no-voter authority.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::TerminalManagement { .. })
    }
    /// Hash of the exact source authority named by this variant.
    pub fn hash(&self) -> &str {
        match self {
            Self::ActiveMembership {
                membership_authority_hash,
            } => membership_authority_hash,
            Self::TerminalManagement {
                terminal_authority_hash,
                ..
            } => terminal_authority_hash,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ReadmissionPreview {
    pub request: ReadmissionRequest,
    pub target: ReadmissionTarget,
    pub principal: String,
    /// Restored historical role; current Disabled registrations are not voters.
    pub metadata_member: bool,
    pub configuration_digest: String,
    pub preview_digest: String,
    pub authority: ReadmissionAuthority,
    pub registry_root_hash: String,
    #[serialization(with = "crate::u64_string")]
    pub registry_generation: u64,
    #[serialization(with = "crate::u64_string")]
    pub source_sequence: u64,
    #[serialization(with = "crate::u64_string")]
    pub topology_generation: u64,
    pub snapshot_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ReadmissionCommit {
    pub preview: ReadmissionPreview,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ReadmissionResume {
    pub operation_id: OperationId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum ReadmissionPhase {
    Prepared,
    Fencing,
    Archiving,
    Activating,
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum ReadmissionState {
    Running,
    RecoveryRequired,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ReadmissionFailure {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ReadmissionStatus {
    pub operation_id: OperationId,
    pub state: ReadmissionState,
    pub phase: ReadmissionPhase,
    pub preview: ReadmissionPreview,
    #[serialization(deserialize_with = "required_nullable")]
    pub failure: Option<ReadmissionFailure>,
    /// Only the core's exact publication after new I/O epoch admission is success.
    #[serialization(deserialize_with = "required_nullable")]
    pub result: Option<OperationResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ReadmissionStatusResponse {
    #[serialization(deserialize_with = "required_nullable")]
    pub operation: Option<ReadmissionStatus>,
}

/// Registration intent is independent of a device's observed online/health state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum RegistrationMode {
    Enabled,
    Disabled,
}

/// A metadata-only projection of the role this authority can restore. This is
/// distinct from the current role: every Disabled registration is a nonvoter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum ReadmissionRole {
    MetadataMember,
    DataOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct RegistrationEntry {
    pub slot_id: String,
    pub disk_id: String,
    #[serialization(with = "crate::u64_string")]
    pub binding_generation: u64,
    pub mode: RegistrationMode,
    pub metadata_member: bool,
    /// Null means the current authority does not offer this readmission lane.
    #[serialization(deserialize_with = "required_nullable")]
    pub readmission: Option<ReadmissionRole>,
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: data_serialization::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "authority", rename_all = "snake_case", deny_unknown_fields)]
pub enum RegistrationSnapshot {
    ActiveMembership {
        registrations: Vec<RegistrationEntry>,
    },
    TerminalManagement {
        registrations: Vec<RegistrationEntry>,
    },
    /// No admitted registry is currently available. Empty is not an empty pool.
    Unadmitted,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/storage_readmission_tests.rs"]
mod tests;
