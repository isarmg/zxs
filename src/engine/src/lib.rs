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
mod authority_selection;
mod business;
pub use business::FileHistoryCopyRequest;
mod container_storage;
mod database_install;
pub use container_storage::DirectoryLayoutReceipt;
mod directory_layout;
pub use directory_layout::LayoutError as DirectoryLayoutError;
mod degraded_recovery;
mod directory_conversion;
pub use directory_conversion::{
    DirectoryConversionConfirmation, DirectoryConversionDigest, DirectoryConversionPreview,
    DirectoryExportOutcome, DirectoryImportFailure, DirectoryImportProgress,
    DirectoryImportStopOutcome, DirectoryImportStopRequest, OrdinaryDirectoryPreview,
    ParseDirectoryConversionDigest, PreparedDirectoryExport, PreparedDirectoryImport,
    PreparedOrdinaryDirectory,
};
mod disk;
pub mod disk_authority;
mod engine;
pub use engine::{
    BackupPreparationPhase, BackupPreparationRequest, BackupPreparationStatus, RecoveryDataRoot,
};
pub use engine::{
    TerminalMetadataResume, TerminalMetadataResumeAdmission, TerminalMetadataResumeCancelOutcome,
    TerminalMetadataResumeCancellationPhase, TerminalMetadataResumePhase,
    TerminalMetadataResumeRecovery, TerminalMetadataResumeStatus, TerminalReadmissionPreview,
};
pub mod file_deletion;
pub use file_deletion::{
    DeleteFileRequest, FileDeletionNodeKind, FileDeletionTarget, UndoFileDeletionRequest,
};
mod file_stamp;
mod foreign_import;
pub use foreign_import::{
    ConfirmedForeignImportIntent, ForeignCheckpoint, ForeignEvidenceDigest, ForeignHistory,
    ForeignImportIntentInfo, ForeignImportIntentRequest, ForeignImportIntentSource,
    ForeignImportIntentTarget, ForeignImportLimits, ForeignImportMapping, ForeignImportPreview,
    ForeignLocalBinding, ForeignMappedHistory, ForeignMigrationPlan, ForeignMigrationReceipt,
    ForeignMigrationTransaction, ForeignPrimaryOrigin, ForeignSourceIdentity,
    ForeignVersionAvailability, PreparedForeignImportIntent,
};
mod historical_save;
pub use engine::{
    DeregistrationNamespace, DeregistrationPlanError, DeregistrationPreview,
    DirectoryNamespaceView, DiskExitPhase, DiskExitStatus, DiskReadmissionPhase,
    DiskReadmissionStatus, DiskRegistrationIdentity, DiskRegistryCommitment,
    FrozenDeregistrationBoundary, HistoricalDiskDefinition, ImpactPage, ImpactPageSize,
    NamespaceDeregistrationPreview, NamespaceEntries, NamespaceOccurrenceImpact,
    NamespaceOccurrenceScan, NamespaceSnapshot, NamespaceSnapshotWitness, OccurrenceVisibility,
    PreparedDiskRegistry, RegisteredDisk, RegisteredHolders, RegistrationEffect, RegistrationMode,
    RegistryExitEffect, RegistryExitRequest, TargetPlacementRole, TerminalDiskExit,
    TerminalManagement, TerminalManagementStatus, TerminalRegistrationState, TerminalRegistry,
    VerifiedDiskRegistry, VerifiedRegistryCandidate, VerifiedRegistryPublication,
    VersionDeregistrationImpact, VersionImpactScan,
};
pub use engine::{
    DirectoryHistoryRead, InitialActivationProbeGuard, PreparedWorkspaceCommit,
    WorkspaceCommitPreparation,
};
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
    DirectoryBranchPreview, DirectoryCommitRequest, DirectoryDefaultSelection,
    DirectoryGitStatusRead, FileBranchMergeRequest, FileDefaultSelection, FileHistoryMergeRequest,
    FileNodeProjectionRequest, FileNodeSummary, FileNodeTagsUpdateRequest, NamespaceTree,
    PreparedUpload, UploadPreparation, validate_branch_name,
};
pub use degraded_recovery::RecoveryPlan;
pub use disk::{
    DiskFile, DiskObservationAccess, DiskObservationDirectory, DiskObservationMount,
    DiskRoleEvidence, RecognizedDisk, RecordedDiskRole, offline_check, offline_export,
    recognize_disk,
};
#[cfg(feature = "fault-injection")]
pub use engine::DiskQuiesceFault;
pub use engine::{
    ContentMatchRead, ContentReadPlan, DiskObservation, DiskObservationFailure,
    DiskObservationPlan, DiskObservationStage, Engine, FileHistorySourceRead,
    HistoryWindowCancellation, HistoryWindowRead, IndexFailure, IndexFailureStage, IndexHealth,
    IntegrityScrubArea, IntegrityScrubBatch, IntegrityScrubBatchAdmission,
    IntegrityScrubCancellation, IntegrityScrubCompletion, IntegrityScrubIssue,
    IntegrityScrubOutcome, IntegrityScrubPlan, IntegrityScrubReport, IntegrityScrubSchedule,
    IntegrityScrubTarget, MembershipAbortStatus, MembershipArchiveInspection,
    MembershipControlArchive, MembershipExitAction, MembershipFenceStatus,
    MembershipJointDecisionStatus, MembershipJointRole, MembershipJointVote,
    MembershipJournalInspection, MetadataMemberProbe, MetadataView, ObservedDiskStatus,
    OpenedContent, OperationReadObservation, OperationReadPlan, OperationRecord,
    PendingMemberControlReceipt, PendingMemberDirectory, PendingMembershipStatus, RangeContent,
    ReadFile, ReadLease, RecoverySealContext, RecoverySealFailure, RecoveryWarnings,
    RetainedWorkspaceLimits, SharedHistoryRead, ValidatedConfig, WorkspaceInventory,
    WorkspaceInventoryRecord, WorkspaceReadPlan, verify_membership_control_archive,
};
pub use history_package::{HistoryManifest, HistoryPackageReport, offline_check_history_package};
pub use protection::{
    BackupConfigurationRequest, PlacementConfigurationRequest, PlacementRuleUpdate,
    ProtectionPolicyUpdate, PublishedBackupDirectory, RecursiveBackupCapture,
    RecursiveBackupConfigurationRequest, ScopedBackupConfigurationRequest,
};
pub use storage_contract::*;
pub use wire::{commit_request_digest, manifest_hash as workspace_manifest_hash};
#[cfg(test)]
#[path = "../../../test/engine/unit/journal/r29_local_publication.rs"]
mod r29_local_publication;
#[cfg(test)]
#[path = "../../../test/engine/unit/repair/io_tail_support.rs"]
mod repair_io_tail_support;
#[cfg(test)]
#[path = "../../../test/engine/unit/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../test/engine/support/directory_git_fixture.rs"]
mod directory_git_fixture;
