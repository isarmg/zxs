//! Single-writer full-file storage, durable publication, and offline recovery.
//!
//! This is a trusted local domain API, not an authentication boundary. Servers
//! authenticate the sole administrator before calling it. Operation principals
//! bind audit authorship, workspace ownership and idempotency, never repository ACLs.
#![forbid(unsafe_code)]
pub use storage_contract::offline_maintenance;
mod error;
pub use error::{
    CheckpointError, Error, PublishedRecoveryReconciliation, PublishedWorkspaceReconciliation,
    RecoveryReconciliationStage,
};
pub type Result<T> = std::result::Result<T, Error>;
mod authority_selection;
mod business;
mod route_topology;
pub use business::{
    CurrentFileHistoryMove, CurrentFileHistoryMoveProjection, FileHistoryCopyRequest,
    FileHistoryMoveCapture, FileHistoryMoveProjection, FileHistoryMoveReplacement,
    FileHistoryMoveRequest, FileHistoryMoveSourceDigest,
};
mod container_storage;
mod database_install;
mod sqlite_connection;
mod sqlite_integrity;
pub use container_storage::{
    DirectoryLayoutReceipt, MAX_OBJECT_DATABASE_ARCHIVE_BYTES, ObjectDatabaseMaterial,
    ObjectDatabaseReconstruction, ObjectDatabaseRecord, ObjectDatabaseScope,
    inspect_object_database_material, reconstruct_object_database_material,
};
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
pub use engine::DetachedHistoryRead;
pub use engine::FileHistoryMoveTargetReport;
pub use engine::{
    BackupCommonDatabasesReport, BackupConfigurationReport, BackupCurrentReadReport,
    BackupDatabaseAlias, BackupDatabaseAliasesReport, BackupDatabaseFile, BackupDatabaseRange,
    BackupDatabaseRead, BackupDatabaseScope, BackupDecidedTargets, BackupDirectoryWriterRoute,
    BackupFileWriterRoute, BackupFirstCandidateReport, BackupFirstFinalizeReport,
    BackupFirstPublicationHash, BackupFirstPublicationReport, BackupGenerationAuthorityReport,
    BackupGenerationControlReport, BackupGenerationPreparationReport, BackupNamespacePage,
    BackupNamespaceScope, BackupNativeMetadataReport, BackupOrdinaryServingReport,
    BackupOrdinaryServingTargetReport, BackupOriginalControlMemberReport,
    BackupOriginalControlReport, BackupOriginalDatabaseReport, BackupOriginalJournalReport,
    BackupPublishedRegistryReport, BackupPublishedScopeTargetReport, BackupPublishedScopesReport,
    BackupRecoveredContent, BackupRegistryDecisionHash, BackupRegistryRecoveryReport,
    BackupRootDatabasesReport, BackupServingContent, BackupServingImagesReport, BackupServingRead,
    BackupServingTargetReport, BackupSystemDatabaseReport, BackupTargetLocatorReport,
    BackupTargetPublicationReport, BackupTargetSealReport, BackupTargetSealsReport,
    BackupTargetWriterIoReport, BackupTargetWriterReport, BackupTargetWritersReport,
    BackupWriterDestination, BackupWriterIoReport, BackupWriterRoutesReport,
    open_decided_backup_targets,
};
pub use engine::{
    BackupCutoverPreparationReport, BackupCutoverPreparationRequest, BackupNativeControlReport,
    BackupNativePreparationReport, BackupNativeTargetReport, BackupObjectDatabasePreparationReport,
    BackupPreparationBatchReport, BackupPreparationPhase, BackupPreparationReport,
    BackupPreparationRequest, BackupPreparationStatus, BackupPreparedTopologyNode,
    BackupScopedDatabasePreparationReport, BackupTopologyTarget, RecoveryDataRoot,
    offline_check_backup_cutover_inputs, offline_check_backup_native_preparation,
    offline_check_backup_object_databases, offline_check_backup_preparation,
    offline_check_backup_preparation_batch, offline_check_backup_scoped_databases,
};
pub use engine::{
    BackupTransactionCancelOutcome, BackupTransactionCancellation, BackupTransactionRequest,
};
pub use engine::{
    CapturedFileMoveSource, FileHistoryMoveCandidatePreparation, FileHistoryMoveDecisionDigest,
    FileHistoryMoveDecisionInputs, FileHistoryMoveDecisionObligation, FileHistoryMoveJointChoice,
    FileHistoryMoveJournalPreparation, FileHistoryMoveNativePreparation,
    FileHistoryMoveOrdinaryPreparation, FileHistoryMovePathRequest,
    FileHistoryMoveScopedPreparation,
};
pub use engine::{FileHistoryMoveRecoveryDigest, FileHistoryMoveRecoveryInfo};
pub use engine::{
    TerminalChainActivationPlanReport, TerminalChainFirstPublication,
    TerminalChainGenerationLogReport, TerminalChainGenerationSelectionReport,
    TerminalChainIoDecisionReport, TerminalChainPublishedCurrent,
    TerminalChainPublishedCurrentReport, TerminalChainReadmissionActivation,
    TerminalChainReadmissionPhase, TerminalChainReadmissionStatus, TerminalChainSelectedGeneration,
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
    ConfirmedForeignImportIntent, ForeignCheckpoint, ForeignDatabaseStatus, ForeignEvidenceDigest,
    ForeignHistory, ForeignHistoryGraphReport, ForeignHistoryGraphStatus, ForeignIdentityBaseline,
    ForeignIdentityClosureReport, ForeignIdentityClosureStatus, ForeignIdentityTarget,
    ForeignIdentityTargetRequest, ForeignImportIntentInfo, ForeignImportIntentRequest,
    ForeignImportIntentSource, ForeignImportIntentTarget, ForeignImportLimits,
    ForeignImportMapping, ForeignImportPreview, ForeignLocalBinding, ForeignMappedHistory,
    ForeignMigrationPlan, ForeignMigrationReceipt, ForeignMigrationStatus,
    ForeignMigrationTransaction, ForeignOriginalDatabasesReport, ForeignOriginalStateReport,
    ForeignPrimaryOrigin, ForeignSourceIdentity, ForeignVersionAvailability,
    PreparedForeignImportIntent,
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
    RegistryExitEffect, RegistryExitRequest, TargetPlacementRole,
    TerminalChainReadmissionPreparation, TerminalChainReadmissionProposal, TerminalDiskExit,
    TerminalManagement, TerminalManagementStatus, TerminalRegistrationState, TerminalRegistry,
    TerminalRetirementArchive, TerminalRetirementCheckpoint, TerminalRetirementPhase,
    TerminalRetirementPlan, TerminalRetirementStatus, VerifiedDiskRegistry,
    VerifiedRegistryCandidate, VerifiedRegistryPublication, VersionDeregistrationImpact,
    VersionImpactScan,
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
// Keep the private authority implementation distinct from the public wire
// module re-exported by storage_contract below.
#[path = "node_retirement.rs"]
mod node_retirement_engine;
pub use node_retirement_engine::PermanentNodeSeparation;
mod protection;
mod publication_evidence;
mod publication_shape;
mod repair;
mod shared_file_topology;
mod state_projection;
mod wire;
pub use business::{
    CurrentObject, DirectoryBranchPreview, DirectoryCommitRequest, DirectoryDefaultSelection,
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
    ContentMatchRead, ContentReadPlan, CoupledNodeRetirement, DiskCatalogDirectory,
    DiskCatalogFailure, DiskCatalogMaintenanceState, DiskCatalogObservation, DiskCatalogReceipt,
    DiskCatalogRefreshOutcome, DiskCatalogRefreshReport, DiskCatalogRefreshResult, DiskCatalogRole,
    DiskCatalogSnapshot, DiskCatalogStatus, DiskCatalogVersion, DiskDatabaseMaterial,
    DiskDatabaseReadScope, DiskDatabaseRebuildSource, DiskDatabaseReconstruction,
    DiskDatabaseRelation, DiskHistoryChange, DiskHistoryFailure, DiskHistoryHeader,
    DiskHistoryMaintenanceState, DiskHistoryObservation, DiskHistoryReceipt, DiskHistoryRecord,
    DiskHistorySnapshot, DiskHistoryStatus, DiskObservation, DiskObservationFailure,
    DiskObservationPlan, DiskObservationStage, Engine, FileHistorySourceRead,
    HistoryWindowCancellation, HistoryWindowRead, IndexFailure, IndexFailureStage, IndexHealth,
    IntegrityScrubArea, IntegrityScrubBatch, IntegrityScrubBatchAdmission,
    IntegrityScrubCancellation, IntegrityScrubCompletion, IntegrityScrubIssue,
    IntegrityScrubOutcome, IntegrityScrubPlan, IntegrityScrubReport, IntegrityScrubSchedule,
    IntegrityScrubTarget, MAX_DISK_DATABASE_ARCHIVE_BYTES, MembershipAbortStatus,
    MembershipArchiveInspection, MembershipControlArchive, MembershipExitAction,
    MembershipFenceStatus, MembershipJointDecisionStatus, MembershipJointRole, MembershipJointVote,
    MembershipJournalInspection, MetadataMemberProbe, MetadataView, ObservedDiskStatus,
    OpenedContent, OperationReadObservation, OperationReadPlan, OperationRecord,
    PendingMemberControlReceipt, PendingMemberDirectory, PendingMembershipStatus,
    PermanentNodeRemovalAnalysis, PermanentNodeRemovalSelection, PermanentNodeRetirementOutcome,
    PermanentRemovalCleanupObservation, RangeContent, ReadFile, ReadLease, RecoverySealContext,
    RecoverySealFailure, RecoveryWarnings, RemovalCleanupScope, RemovalParentProjection,
    RemovalReplicaCleanup, RemovalReplicaCleanupState, RetainedWorkspaceLimits, SharedHistoryRead,
    SystemAuditEvent, SystemCatalogKind, SystemCatalogRecord, SystemCatalogValue,
    SystemChangeCursor, SystemChangePage, SystemChangeReadError, SystemOperationPrincipal,
    SystemStateMetadata, ValidatedConfig, WorkspaceInventory, WorkspaceInventoryRecord,
    WorkspaceReadPlan, inspect_disk_database_material, reconstruct_disk_database_material,
    verify_membership_control_archive,
};
pub use history_package::{HistoryManifest, HistoryPackageReport, offline_check_history_package};
/// Canonical disk-slot syntax for trusted local maintenance request boundaries.
/// This validates only a name, never a disk identity or permission to admit it.
pub use name_policy::validate_slot;
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
