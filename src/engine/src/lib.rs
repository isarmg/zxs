//! Single-writer full-file storage, durable publication, and offline recovery.
//!
//! This is a trusted local domain API, not an authentication boundary. Servers
//! authenticate the sole administrator before calling it. Operation principals
//! bind audit authorship, workspace ownership and idempotency, never repository ACLs.
#![forbid(unsafe_code)]
mod error;
pub use error::{
    CheckpointError, Error, PublishedRecoveryReconciliation, PublishedWorkspaceReconciliation,
    RecoveryReconciliationStage,
};
pub type Result<T> = std::result::Result<T, Error>;
mod business;
mod container_storage;
mod degraded_recovery;
mod disk;
mod engine;
mod file_stamp;
mod historical_save;
pub use engine::{
    DeregistrationNamespace, DeregistrationPlanError, DeregistrationPreview,
    DiskRegistrationIdentity, DiskRegistryCommitment, FrozenDeregistrationBoundary,
    HistoricalDiskDefinition, ImpactPage, ImpactPageSize, NamespaceDeregistrationPreview,
    NamespaceOccurrenceImpact, NamespaceOccurrenceScan, OccurrenceVisibility, PreparedDiskRegistry,
    RegisteredDisk, RegisteredHolders, RegistrationEffect, RegistrationMode, RegistryExitEffect,
    RegistryExitRequest, TargetPlacementRole, VerifiedDiskRegistry, VerifiedRegistryCandidate,
    VerifiedRegistryPublication, VersionDeregistrationImpact, VersionImpactScan,
};
pub use engine::{PreparedWorkspaceCommit, WorkspaceCommitPreparation};
mod history_package;
mod journal;
#[cfg(feature = "fault-injection")]
pub use journal::JournalFault;
mod maintenance;
mod metadata_budget;
mod metadata_map;
mod protection;
mod publication_evidence;
mod repair;
mod state_projection;
mod wire;
pub use business::{
    DirectoryBranchPreview, DirectoryDefaultSelection, FileBranchMergeRequest,
    FileDefaultSelection, FileNodeProjectionRequest, FileNodeSummary, FileNodeTagsUpdateRequest,
    NamespaceTree, PreparedUpload, UploadPreparation, validate_branch_name,
};
pub use degraded_recovery::RecoveryPlan;
pub use disk::{DiskFile, offline_check, offline_export};
#[cfg(feature = "fault-injection")]
pub use engine::DiskQuiesceFault;
pub use engine::{
    ContentReadPlan, DiskObservation, DiskObservationFailure, DiskObservationPlan,
    DiskObservationStage, Engine, HistoryWindowCancellation, HistoryWindowRead, IndexFailure,
    IndexFailureStage, IndexHealth, IntegrityScrubArea, IntegrityScrubBatch,
    IntegrityScrubBatchAdmission, IntegrityScrubCancellation, IntegrityScrubCompletion,
    IntegrityScrubIssue, IntegrityScrubOutcome, IntegrityScrubPlan, IntegrityScrubReport,
    IntegrityScrubSchedule, IntegrityScrubTarget, MembershipArchiveInspection,
    MembershipControlArchive, MembershipExitAction, MembershipFenceStatus,
    MembershipJointDecisionStatus, MembershipJointRole, MembershipJointVote,
    MembershipJournalInspection, MetadataMemberProbe, MetadataView, ObservedDiskStatus,
    OpenedContent, OperationReadObservation, OperationReadPlan, OperationRecord,
    PendingMemberControlReceipt, PendingMemberDirectory, PendingMembershipStatus, RangeContent,
    ReadFile, ReadLease, RecoverySealContext, RecoverySealFailure, RecoveryWarnings,
    RetainedWorkspaceLimits, ValidatedConfig, WorkspaceInventory, WorkspaceInventoryRecord,
    WorkspaceReadPlan, verify_membership_control_archive,
};
pub use history_package::{HistoryManifest, HistoryPackageReport, offline_check_history_package};
pub use protection::{
    BackupConfigurationRequest, PlacementRuleUpdate, ProtectionPolicyUpdate,
    ScopedBackupConfigurationRequest,
};
pub use storage_contract::*;
pub use wire::{commit_request_digest, manifest_hash as workspace_manifest_hash};
#[cfg(test)]
#[path = "../../../test/engine/unit/journal/r29_local_publication.rs"]
mod r29_local_publication;
#[cfg(test)]
#[path = "../../../test/engine/unit/tests.rs"]
mod tests;
