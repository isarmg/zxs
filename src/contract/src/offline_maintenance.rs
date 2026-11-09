//! Versioned, bounded helper-to-matching-program pipe protocol. This is a
//! transport contract only; credential and storage admission stay in the server.
use data_serialization::{Deserialize, Serialize};
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum Operation {
    AdministratorResetPreview,
    AdministratorReset,
    AdministratorResetRecoverPreview,
    FirstAdministratorPreview,
    FirstAdministratorRecoverPreview,
    FirstAdministratorCreate,
    FirstStoragePreview,
    FirstStorageRecover,
    FirstStorageCreate,
    ForeignChainPreview,
    ForeignChainOriginal,
    ForeignChainPrepare,
    ForeignChainDatabasesPreview,
    ForeignChainDatabasesPrepare,
    ForeignChainIdentityPreview,
    DiskAdditionPreview,
    DiskAdditionOriginal,
    DiskAdditionCommit,
    DiskRemovalInventory,
    DiskRemovalPreview,
    DiskRemovalOriginal,
    DiskRemovalCommit,
    TerminalRemovalInspect,
    TerminalRetirementPreview,
    TerminalRetirementOriginal,
    TerminalRetirementCommit,
    DiskReadmissionPreview,
    DiskReadmissionOriginal,
    DiskReadmissionCommit,
    TerminalChainExport,
    TerminalChainReadmissionProposal,
    TerminalChainTargetStatus,
    TerminalChainTargetSynchronize,
    TerminalChainFirstPrepare,
    TerminalChainFirstPublish,
    TerminalChainCurrent,
    TerminalChainActivationPlan,
    TerminalChainIoDecide,
    TerminalChainGenerationLog,
    TerminalChainGenerationSelect,
    TerminalChainGenerationRead,
}
impl Operation {
    pub fn failure_code(self) -> &'static str {
        match self {
            Self::AdministratorResetPreview => "administrator_password_reset",
            Self::AdministratorReset => "administrator_password_reset",
            Self::AdministratorResetRecoverPreview => "administrator_password_reset",
            Self::FirstAdministratorPreview => "first_management_administrator",
            Self::FirstAdministratorRecoverPreview => "first_management_administrator",
            Self::FirstAdministratorCreate => "first_management_administrator",
            Self::FirstStoragePreview => "first_storage_initialization",
            Self::FirstStorageRecover => "first_storage_initialization",
            Self::FirstStorageCreate => "first_storage_initialization",
            Self::ForeignChainPreview => "foreign_chain_material",
            Self::ForeignChainOriginal => "foreign_chain_material",
            Self::ForeignChainPrepare => "foreign_chain_material",
            Self::ForeignChainDatabasesPreview => "foreign_chain_material",
            Self::ForeignChainDatabasesPrepare => "foreign_chain_material",
            Self::ForeignChainIdentityPreview => "foreign_chain_material",
            Self::DiskAdditionPreview => "active_disk_addition",
            Self::DiskAdditionOriginal => "active_disk_addition",
            Self::DiskAdditionCommit => "active_disk_addition",
            Self::DiskRemovalInventory => "active_disk_deregistration",
            Self::DiskRemovalPreview => "active_disk_deregistration",
            Self::DiskRemovalOriginal => "active_disk_deregistration",
            Self::DiskRemovalCommit => "active_disk_deregistration",
            Self::TerminalRemovalInspect => "terminal_removal_inspection",
            Self::TerminalRetirementPreview => "terminal_retirement",
            Self::TerminalRetirementOriginal => "terminal_retirement",
            Self::TerminalRetirementCommit => "terminal_retirement",
            Self::DiskReadmissionPreview => "terminal_readmission",
            Self::DiskReadmissionOriginal => "terminal_readmission",
            Self::DiskReadmissionCommit => "terminal_readmission",
            Self::TerminalChainExport => "terminal_checkpoint",
            Self::TerminalChainReadmissionProposal => "terminal_chain_readmission_proposal",
            Self::TerminalChainTargetStatus => "chain_target_status",
            Self::TerminalChainTargetSynchronize => "chain_target_pending",
            Self::TerminalChainFirstPrepare => "chain_first_inputs_pending",
            Self::TerminalChainFirstPublish => "chain_first_publication_pending",
            Self::TerminalChainCurrent => "chain_published_current",
            Self::TerminalChainActivationPlan => "chain_activation_plan_pending",
            Self::TerminalChainIoDecide => "chain_io_decision_pending",
            Self::TerminalChainGenerationLog => "chain_generation_log_pending",
            Self::TerminalChainGenerationSelect => "chain_generation_select_pending",
            Self::TerminalChainGenerationRead => "chain_generation_read_pending",
        }
    }
    pub fn secret(self) -> bool {
        matches!(
            self,
            Self::AdministratorResetPreview
                | Self::AdministratorReset
                | Self::AdministratorResetRecoverPreview
                | Self::FirstAdministratorPreview
                | Self::FirstAdministratorRecoverPreview
                | Self::FirstAdministratorCreate
        )
    }
    /// Initial storage can fail through credential backup material. Preserve
    /// the existing top-level-only diagnostic policy, distinct from POST secrets.
    pub fn terse_failure(self) -> bool {
        self.secret()
            || matches!(
                self,
                Self::FirstStoragePreview | Self::FirstStorageRecover | Self::FirstStorageCreate
            )
    }
    pub fn response_limit(self) -> usize {
        match self {
            Self::TerminalChainExport | Self::TerminalChainReadmissionProposal => {
                128 * 1024 * 1024 + 64 * 1024
            }
            _ => 64 * 1024,
        }
    }
}
#[derive(Serialize, Deserialize)]
pub enum Format {
    #[serialization(rename = "zxs-offline-maintenance-pipe-1")]
    Current,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct RequestHeader {
    pub format: Format,
    pub operation: Operation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum FailureKind {
    InvalidRequest,
    Conflict,
}
#[derive(Serialize, Deserialize)]
#[serialization(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReplyHeader {
    Success { body_bytes: usize },
    Failed { kind: FailureKind, message: String },
}
