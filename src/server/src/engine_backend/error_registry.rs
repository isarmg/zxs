//! Exhaustive engine-to-HTTP policy. Wire spellings live in ErrorCode.
use storage_contract::ErrorCode;

#[cfg(test)]
pub(super) const ALIASES: &[(ErrorCode, ErrorCode)] = &[
    (ErrorCode::PermissionDenied, ErrorCode::Forbidden),
    (ErrorCode::Unauthenticated, ErrorCode::Unauthorized),
    (ErrorCode::FileVersionConflict, ErrorCode::FileConflict),
    (ErrorCode::BaseHeadMismatch, ErrorCode::HeadConflict),
    (ErrorCode::FileBusy, ErrorCode::FileWriteBusy),
    (ErrorCode::FencingTokenInvalid, ErrorCode::Fenced),
    (ErrorCode::FencingTokenExpired, ErrorCode::LeaseExpired),
    (ErrorCode::ResourceLimit, ErrorCode::ResourceLimitExceeded),
    (ErrorCode::RouteIncomplete, ErrorCode::BackupRouteIncomplete),
    (ErrorCode::InDoubt, ErrorCode::CommitInDoubt),
    (ErrorCode::GenerationMismatch, ErrorCode::ConfigConflict),
    (
        ErrorCode::StaleUploadGeneration,
        ErrorCode::UploadGenerationConflict,
    ),
];

#[cfg(test)]
pub(super) const PUBLIC_STATUS: &[(ErrorCode, u16)] = &[
    (ErrorCode::Unauthorized, 401),
    (ErrorCode::Forbidden, 403),
    (ErrorCode::ContentPurged, 410),
    (ErrorCode::NoSpace, 507),
    (ErrorCode::ResourceLimitExceeded, 429),
    (ErrorCode::QuotaExceeded, 429),
    (ErrorCode::ContentUnavailable, 503),
    (ErrorCode::MetadataUnavailable, 503),
    (ErrorCode::RecoveryRequired, 503),
    (ErrorCode::ReplicaTargetUnavailable, 503),
    (ErrorCode::RouteTargetUnavailable, 503),
    (ErrorCode::CommitInDoubt, 503),
    (ErrorCode::BackupRouteIncomplete, 503),
    (ErrorCode::CommitPreparationBusy, 409),
    (ErrorCode::CommitPreparationStale, 409),
    (ErrorCode::PreparationStale, 409),
    (ErrorCode::HeadConflict, 409),
    (ErrorCode::BranchNotActive, 409),
    (ErrorCode::Fenced, 409),
    (ErrorCode::LeaseExpired, 409),
    (ErrorCode::UploadGenerationConflict, 409),
    (ErrorCode::FileConflict, 409),
    (ErrorCode::FileBranchConflict, 409),
    (ErrorCode::DirectoryBranchConflict, 409),
    (ErrorCode::DirectoryAliasConflict, 409),
    (ErrorCode::BatchMappingConflict, 409),
    (ErrorCode::BatchScopeConflict, 409),
    (ErrorCode::IdempotencyKeyReused, 409),
    (ErrorCode::SplitPlanStale, 409),
    (ErrorCode::PurgeBlockedByReference, 409),
    (ErrorCode::ContentPinned, 409),
    (ErrorCode::ConfigConflict, 409),
    (ErrorCode::BranchWriteBusy, 409),
    (ErrorCode::FileWriteBusy, 409),
    (ErrorCode::NamespaceConflict, 409),
    (ErrorCode::NameCollision, 409),
    (ErrorCode::WorkspaceClosed, 409),
    (ErrorCode::PlanStale, 409),
    (ErrorCode::PlanClosed, 409),
    (ErrorCode::TreeMoved, 409),
    (ErrorCode::RestoreBackupConflict, 409),
    (ErrorCode::DirectoryBranchIncomplete, 409),
    (ErrorCode::DirectoryScopeRequired, 409),
    (ErrorCode::DirectoryHistorySplitUnsupported, 400),
    (ErrorCode::FileNodeHistorySplitUnsupported, 400),
    (ErrorCode::IoError, 500),
    (ErrorCode::DurabilityUnknown, 500),
    (ErrorCode::StateInvalid, 500),
    (ErrorCode::InternalError, 500),
    (ErrorCode::ObjectCorrupt, 500),
    (ErrorCode::LogCorrupt, 500),
    (ErrorCode::PublicationInvalid, 500),
    (ErrorCode::OperationUnsupported, 501),
    (ErrorCode::PlanUnsupported, 501),
];

pub(super) fn public_code(internal: ErrorCode) -> ErrorCode {
    match internal {
        ErrorCode::PermissionDenied => ErrorCode::Forbidden,
        ErrorCode::Unauthenticated => ErrorCode::Unauthorized,
        ErrorCode::FileVersionConflict => ErrorCode::FileConflict,
        ErrorCode::BaseHeadMismatch => ErrorCode::HeadConflict,
        ErrorCode::FileBusy => ErrorCode::FileWriteBusy,
        ErrorCode::FencingTokenInvalid => ErrorCode::Fenced,
        ErrorCode::FencingTokenExpired => ErrorCode::LeaseExpired,
        ErrorCode::ResourceLimit => ErrorCode::ResourceLimitExceeded,
        ErrorCode::RouteIncomplete => ErrorCode::BackupRouteIncomplete,
        ErrorCode::InDoubt => ErrorCode::CommitInDoubt,
        ErrorCode::GenerationMismatch => ErrorCode::ConfigConflict,
        ErrorCode::StaleUploadGeneration => ErrorCode::UploadGenerationConflict,
        ErrorCode::AlreadyInitialized
        | ErrorCode::AuthorityRecoveryRequired
        | ErrorCode::AuthAlreadyInitialized
        | ErrorCode::AuthRecoveryRequired
        | ErrorCode::AuthStateInvalid
        | ErrorCode::AuthStoreBusy
        | ErrorCode::BackupConfigurationRequired
        | ErrorCode::BackupPathConflict
        | ErrorCode::BackupPrefixRequired
        | ErrorCode::BackupRouteIncomplete
        | ErrorCode::BatchMappingConflict
        | ErrorCode::BatchScopeConflict
        | ErrorCode::BootstrapExpired
        | ErrorCode::BranchArchived
        | ErrorCode::BranchNotActive
        | ErrorCode::BranchNotFound
        | ErrorCode::BranchWriteBusy
        | ErrorCode::CheckpointInvalid
        | ErrorCode::ChecksumFormatInvalid
        | ErrorCode::ChecksumMismatch
        | ErrorCode::CommitInDoubt
        | ErrorCode::CommitNotFound
        | ErrorCode::CommitPreparationBusy
        | ErrorCode::CommitPreparationStale
        | ErrorCode::ConfigConflict
        | ErrorCode::ConfigInvalid
        | ErrorCode::ContainerAmbiguous
        | ErrorCode::ContainerLayoutInvalid
        | ErrorCode::ContentCorrupt
        | ErrorCode::ContentMissing
        | ErrorCode::ContentPinned
        | ErrorCode::ContentPurged
        | ErrorCode::ContentUnavailable
        | ErrorCode::CurrentPasswordInvalid
        | ErrorCode::DestinationExists
        | ErrorCode::DirectoryAliasConflict
        | ErrorCode::DirectoryBranchConflict
        | ErrorCode::DirectoryBranchIncomplete
        | ErrorCode::DirectoryBranchNotFound
        | ErrorCode::DirectoryBranchScope
        | ErrorCode::DirectoryHistorySplitUnsupported
        | ErrorCode::DirectoryNotEmpty
        | ErrorCode::DirectoryNotFound
        | ErrorCode::DirectoryScopeRequired
        | ErrorCode::DiskAlreadyEnrolled
        | ErrorCode::DiskChanged
        | ErrorCode::DiskIdentityMismatch
        | ErrorCode::DiskNotFound
        | ErrorCode::DiskNotIndependent
        | ErrorCode::DiskNotMounted
        | ErrorCode::DiskReadOnly
        | ErrorCode::DiskSampleUnavailable
        | ErrorCode::DiskUnavailable
        | ErrorCode::DurabilityUnknown
        | ErrorCode::EntryNotFound
        | ErrorCode::Fenced
        | ErrorCode::FilesystemRequired
        | ErrorCode::FileBranchConflict
        | ErrorCode::FileBranchNotFound
        | ErrorCode::FileBranchRequired
        | ErrorCode::FileBranchScope
        | ErrorCode::FileBranchSelected
        | ErrorCode::FileConflict
        | ErrorCode::FileHistorySplitUnsupported
        | ErrorCode::FileNodeHistorySplitUnsupported
        | ErrorCode::FileNodeNotFound
        | ErrorCode::FileNotFound
        | ErrorCode::FileTooLarge
        | ErrorCode::FileWriteBusy
        | ErrorCode::Forbidden
        | ErrorCode::FormatUnsupported
        | ErrorCode::HeadConflict
        | ErrorCode::HistoricalSaveRequiresBranch
        | ErrorCode::HistoryMismatch
        | ErrorCode::HistoryNodeNotFound
        | ErrorCode::HistoryNotFound
        | ErrorCode::HistoryPackageInvalid
        | ErrorCode::HttpError
        | ErrorCode::HttpUploadsPending
        | ErrorCode::HttpUploadsUnavailable
        | ErrorCode::IdempotencyKeyReused
        | ErrorCode::IndexRebuildFailed
        | ErrorCode::InjectedFailure
        | ErrorCode::InternalError
        | ErrorCode::InvalidAdminResponse
        | ErrorCode::InvalidApiPath
        | ErrorCode::InvalidBackupPrefix
        | ErrorCode::InvalidCursor
        | ErrorCode::InvalidDirectoryPage
        | ErrorCode::InvalidDiskList
        | ErrorCode::InvalidId
        | ErrorCode::InvalidInteger
        | ErrorCode::InvalidJson
        | ErrorCode::InvalidMove
        | ErrorCode::InvalidObjectType
        | ErrorCode::InvalidRequest
        | ErrorCode::InvalidResponse
        | ErrorCode::InvalidSession
        | ErrorCode::InvalidSetupRequest
        | ErrorCode::InvalidSlotId
        | ErrorCode::InvalidTag
        | ErrorCode::IoError
        | ErrorCode::IsDirectory
        | ErrorCode::ItemNotFound
        | ErrorCode::ItemPathMismatch
        | ErrorCode::LeaseExpired
        | ErrorCode::ListingChanged
        | ErrorCode::LogCorrupt
        | ErrorCode::LogFork
        | ErrorCode::LogFormatUnsupported
        | ErrorCode::MainBranchRequired
        | ErrorCode::MergeEndpointMismatch
        | ErrorCode::MetadataQuorumUnavailable
        | ErrorCode::MetadataTooLong
        | ErrorCode::MetadataUnavailable
        | ErrorCode::MethodNotAllowed
        | ErrorCode::MonitorUnavailable
        | ErrorCode::MultiDeviceFilesystem
        | ErrorCode::NamespaceConflict
        | ErrorCode::NameCollision
        | ErrorCode::NameConflict
        | ErrorCode::NameInvalidCharacter
        | ErrorCode::NameNotNormalized
        | ErrorCode::NameReserved
        | ErrorCode::NameTooLong
        | ErrorCode::NotDirectory
        | ErrorCode::NotFound
        | ErrorCode::NoSpace
        | ErrorCode::NoWindow
        | ErrorCode::ObjectCollision
        | ErrorCode::ObjectCorrupt
        | ErrorCode::OfflineConfigurationRequired
        | ErrorCode::OperationAborted
        | ErrorCode::OperationIdConflict
        | ErrorCode::OperationNotFound
        | ErrorCode::OperationUnsupported
        | ErrorCode::ParentNotDirectory
        | ErrorCode::PathInvalid
        | ErrorCode::PathTooDeep
        | ErrorCode::PayloadTooLarge
        | ErrorCode::PermanentLossNotAcknowledged
        | ErrorCode::PlacementInvalid
        | ErrorCode::PlacementNotFound
        | ErrorCode::PlanClosed
        | ErrorCode::PlanInvalid
        | ErrorCode::PlanNotFound
        | ErrorCode::PlanStale
        | ErrorCode::PlanUnsupported
        | ErrorCode::PreparationStale
        | ErrorCode::PublicationInvalid
        | ErrorCode::PurgeBlockedByReference
        | ErrorCode::QuotaExceeded
        | ErrorCode::RangeNotSatisfiable
        | ErrorCode::RecoveryPlanNotFound
        | ErrorCode::RecoveryRequired
        | ErrorCode::RecoveryWorkerFailed
        | ErrorCode::ReplicaTargetUnavailable
        | ErrorCode::RepositoryNotFound
        | ErrorCode::RequestBindingMismatch
        | ErrorCode::RequestDigestMismatch
        | ErrorCode::RequestTimeout
        | ErrorCode::ResourceLimitExceeded
        | ErrorCode::ResponseTooLarge
        | ErrorCode::RestoreBackupConflict
        | ErrorCode::RouteTargetUnavailable
        | ErrorCode::SaveModeUnsupported
        | ErrorCode::ScopeChanged
        | ErrorCode::SealedManifestMismatch
        | ErrorCode::ServiceStopping
        | ErrorCode::SessionChanged
        | ErrorCode::SetupBusy
        | ErrorCode::SetupContentChanged
        | ErrorCode::SetupPresetChanged
        | ErrorCode::SetupProofInvalid
        | ErrorCode::SlotNotFound
        | ErrorCode::SmbRebindRequired
        | ErrorCode::SmbSpoolUnavailable
        | ErrorCode::SmbUnavailable
        | ErrorCode::SourceChanged
        | ErrorCode::SplitInvalid
        | ErrorCode::SplitPlanStale
        | ErrorCode::SplitRootForbidden
        | ErrorCode::StateInvalid
        | ErrorCode::StorageActive
        | ErrorCode::StorageDraining
        | ErrorCode::StorageRetryRunning
        | ErrorCode::StorageUnavailable
        | ErrorCode::SystemAuthRequired
        | ErrorCode::SystemRootChanged
        | ErrorCode::TagLimitExceeded
        | ErrorCode::TombstoneInvalid
        | ErrorCode::TopologyGenerationMismatch
        | ErrorCode::TopologyInvalid
        | ErrorCode::TransferTimeout
        | ErrorCode::TransportError
        | ErrorCode::TreeMoved
        | ErrorCode::Unauthorized
        | ErrorCode::UnsafePath
        | ErrorCode::UnsupportedDeviceTopology
        | ErrorCode::UnsupportedMediaType
        | ErrorCode::UnsupportedOperation
        | ErrorCode::UploadGenerationConflict
        | ErrorCode::UploadIncomplete
        | ErrorCode::UploadRangeConflict
        | ErrorCode::UploadStateInvalid
        | ErrorCode::VersionNotFound
        | ErrorCode::WebAssetsUnavailable
        | ErrorCode::WorkspaceBusy
        | ErrorCode::WorkspaceChanged
        | ErrorCode::WorkspaceClosed
        | ErrorCode::WorkspaceDependencyBusy
        | ErrorCode::WorkspaceNotFound
        | ErrorCode::WorkspaceNotSealed
        | ErrorCode::WorkspaceRebaseRequired
        | ErrorCode::WriterAlreadyActive => internal,
    }
}

pub(super) fn status(code: ErrorCode) -> u16 {
    match code {
        ErrorCode::AlreadyInitialized
        | ErrorCode::AuthorityRecoveryRequired
        | ErrorCode::AuthAlreadyInitialized
        | ErrorCode::AuthRecoveryRequired
        | ErrorCode::AuthStateInvalid
        | ErrorCode::AuthStoreBusy
        | ErrorCode::BackupConfigurationRequired
        | ErrorCode::BackupPathConflict
        | ErrorCode::BackupPrefixRequired
        | ErrorCode::BaseHeadMismatch
        | ErrorCode::BootstrapExpired
        | ErrorCode::BranchArchived
        | ErrorCode::CheckpointInvalid
        | ErrorCode::ChecksumFormatInvalid
        | ErrorCode::ChecksumMismatch
        | ErrorCode::ConfigInvalid
        | ErrorCode::ContainerAmbiguous
        | ErrorCode::ContainerLayoutInvalid
        | ErrorCode::ContentCorrupt
        | ErrorCode::ContentMissing
        | ErrorCode::CurrentPasswordInvalid
        | ErrorCode::DestinationExists
        | ErrorCode::DirectoryBranchScope
        | ErrorCode::DirectoryHistorySplitUnsupported
        | ErrorCode::DirectoryNotEmpty
        | ErrorCode::DiskAlreadyEnrolled
        | ErrorCode::DiskChanged
        | ErrorCode::DiskIdentityMismatch
        | ErrorCode::DiskNotIndependent
        | ErrorCode::DiskNotMounted
        | ErrorCode::DiskReadOnly
        | ErrorCode::DiskSampleUnavailable
        | ErrorCode::DiskUnavailable
        | ErrorCode::FencingTokenExpired
        | ErrorCode::FencingTokenInvalid
        | ErrorCode::FilesystemRequired
        | ErrorCode::FileBranchRequired
        | ErrorCode::FileBranchScope
        | ErrorCode::FileBranchSelected
        | ErrorCode::FileBusy
        | ErrorCode::FileHistorySplitUnsupported
        | ErrorCode::FileNodeHistorySplitUnsupported
        | ErrorCode::FileTooLarge
        | ErrorCode::FileVersionConflict
        | ErrorCode::FormatUnsupported
        | ErrorCode::GenerationMismatch
        | ErrorCode::HistoricalSaveRequiresBranch
        | ErrorCode::HistoryMismatch
        | ErrorCode::HistoryPackageInvalid
        | ErrorCode::HttpError
        | ErrorCode::HttpUploadsPending
        | ErrorCode::HttpUploadsUnavailable
        | ErrorCode::IndexRebuildFailed
        | ErrorCode::InjectedFailure
        | ErrorCode::InvalidAdminResponse
        | ErrorCode::InvalidApiPath
        | ErrorCode::InvalidBackupPrefix
        | ErrorCode::InvalidCursor
        | ErrorCode::InvalidDirectoryPage
        | ErrorCode::InvalidDiskList
        | ErrorCode::InvalidId
        | ErrorCode::InvalidInteger
        | ErrorCode::InvalidJson
        | ErrorCode::InvalidMove
        | ErrorCode::InvalidObjectType
        | ErrorCode::InvalidRequest
        | ErrorCode::InvalidResponse
        | ErrorCode::InvalidSession
        | ErrorCode::InvalidSetupRequest
        | ErrorCode::InvalidSlotId
        | ErrorCode::InvalidTag
        | ErrorCode::InDoubt
        | ErrorCode::IsDirectory
        | ErrorCode::ItemPathMismatch
        | ErrorCode::ListingChanged
        | ErrorCode::LogFork
        | ErrorCode::LogFormatUnsupported
        | ErrorCode::MainBranchRequired
        | ErrorCode::MergeEndpointMismatch
        | ErrorCode::MetadataQuorumUnavailable
        | ErrorCode::MetadataTooLong
        | ErrorCode::MethodNotAllowed
        | ErrorCode::MonitorUnavailable
        | ErrorCode::MultiDeviceFilesystem
        | ErrorCode::NameConflict
        | ErrorCode::NameInvalidCharacter
        | ErrorCode::NameNotNormalized
        | ErrorCode::NameReserved
        | ErrorCode::NameTooLong
        | ErrorCode::NotDirectory
        | ErrorCode::NoWindow
        | ErrorCode::ObjectCollision
        | ErrorCode::OfflineConfigurationRequired
        | ErrorCode::OperationAborted
        | ErrorCode::OperationIdConflict
        | ErrorCode::ParentNotDirectory
        | ErrorCode::PathInvalid
        | ErrorCode::PathTooDeep
        | ErrorCode::PayloadTooLarge
        | ErrorCode::PermanentLossNotAcknowledged
        | ErrorCode::PermissionDenied
        | ErrorCode::PlacementInvalid
        | ErrorCode::PlanInvalid
        | ErrorCode::RangeNotSatisfiable
        | ErrorCode::RecoveryWorkerFailed
        | ErrorCode::RequestBindingMismatch
        | ErrorCode::RequestDigestMismatch
        | ErrorCode::RequestTimeout
        | ErrorCode::ResourceLimit
        | ErrorCode::ResponseTooLarge
        | ErrorCode::RouteIncomplete
        | ErrorCode::SaveModeUnsupported
        | ErrorCode::ScopeChanged
        | ErrorCode::SealedManifestMismatch
        | ErrorCode::ServiceStopping
        | ErrorCode::SessionChanged
        | ErrorCode::SetupBusy
        | ErrorCode::SetupContentChanged
        | ErrorCode::SetupPresetChanged
        | ErrorCode::SetupProofInvalid
        | ErrorCode::SmbRebindRequired
        | ErrorCode::SmbSpoolUnavailable
        | ErrorCode::SmbUnavailable
        | ErrorCode::SourceChanged
        | ErrorCode::SplitInvalid
        | ErrorCode::SplitRootForbidden
        | ErrorCode::StaleUploadGeneration
        | ErrorCode::StorageActive
        | ErrorCode::StorageDraining
        | ErrorCode::StorageRetryRunning
        | ErrorCode::StorageUnavailable
        | ErrorCode::SystemAuthRequired
        | ErrorCode::SystemRootChanged
        | ErrorCode::TagLimitExceeded
        | ErrorCode::TombstoneInvalid
        | ErrorCode::TopologyGenerationMismatch
        | ErrorCode::TopologyInvalid
        | ErrorCode::TransferTimeout
        | ErrorCode::TransportError
        | ErrorCode::Unauthenticated
        | ErrorCode::UnsafePath
        | ErrorCode::UnsupportedDeviceTopology
        | ErrorCode::UnsupportedMediaType
        | ErrorCode::UnsupportedOperation
        | ErrorCode::UploadIncomplete
        | ErrorCode::UploadRangeConflict
        | ErrorCode::UploadStateInvalid
        | ErrorCode::WebAssetsUnavailable
        | ErrorCode::WorkspaceBusy
        | ErrorCode::WorkspaceChanged
        | ErrorCode::WorkspaceDependencyBusy
        | ErrorCode::WorkspaceNotSealed
        | ErrorCode::WorkspaceRebaseRequired
        | ErrorCode::WriterAlreadyActive => 400,
        ErrorCode::Unauthorized => 401,
        ErrorCode::Forbidden => 403,
        ErrorCode::BranchNotFound
        | ErrorCode::CommitNotFound
        | ErrorCode::DirectoryBranchNotFound
        | ErrorCode::DirectoryNotFound
        | ErrorCode::DiskNotFound
        | ErrorCode::EntryNotFound
        | ErrorCode::FileBranchNotFound
        | ErrorCode::FileNodeNotFound
        | ErrorCode::FileNotFound
        | ErrorCode::HistoryNodeNotFound
        | ErrorCode::HistoryNotFound
        | ErrorCode::ItemNotFound
        | ErrorCode::NotFound
        | ErrorCode::OperationNotFound
        | ErrorCode::PlacementNotFound
        | ErrorCode::PlanNotFound
        | ErrorCode::RecoveryPlanNotFound
        | ErrorCode::RepositoryNotFound
        | ErrorCode::SlotNotFound
        | ErrorCode::VersionNotFound
        | ErrorCode::WorkspaceNotFound => 404,
        ErrorCode::BatchMappingConflict
        | ErrorCode::BatchScopeConflict
        | ErrorCode::BranchNotActive
        | ErrorCode::BranchWriteBusy
        | ErrorCode::CommitPreparationBusy
        | ErrorCode::CommitPreparationStale
        | ErrorCode::ConfigConflict
        | ErrorCode::ContentPinned
        | ErrorCode::DirectoryAliasConflict
        | ErrorCode::DirectoryBranchConflict
        | ErrorCode::DirectoryBranchIncomplete
        | ErrorCode::DirectoryScopeRequired
        | ErrorCode::Fenced
        | ErrorCode::FileBranchConflict
        | ErrorCode::FileConflict
        | ErrorCode::FileWriteBusy
        | ErrorCode::HeadConflict
        | ErrorCode::IdempotencyKeyReused
        | ErrorCode::LeaseExpired
        | ErrorCode::NamespaceConflict
        | ErrorCode::NameCollision
        | ErrorCode::PlanClosed
        | ErrorCode::PlanStale
        | ErrorCode::PreparationStale
        | ErrorCode::PurgeBlockedByReference
        | ErrorCode::RestoreBackupConflict
        | ErrorCode::SplitPlanStale
        | ErrorCode::TreeMoved
        | ErrorCode::UploadGenerationConflict
        | ErrorCode::WorkspaceClosed => 409,
        ErrorCode::ContentPurged => 410,
        ErrorCode::QuotaExceeded | ErrorCode::ResourceLimitExceeded => 429,
        ErrorCode::DurabilityUnknown
        | ErrorCode::InternalError
        | ErrorCode::IoError
        | ErrorCode::LogCorrupt
        | ErrorCode::ObjectCorrupt
        | ErrorCode::PublicationInvalid
        | ErrorCode::StateInvalid => 500,
        ErrorCode::OperationUnsupported | ErrorCode::PlanUnsupported => 501,
        ErrorCode::BackupRouteIncomplete
        | ErrorCode::CommitInDoubt
        | ErrorCode::ContentUnavailable
        | ErrorCode::MetadataUnavailable
        | ErrorCode::RecoveryRequired
        | ErrorCode::ReplicaTargetUnavailable
        | ErrorCode::RouteTargetUnavailable => 503,
        ErrorCode::NoSpace => 507,
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/error_registry.rs"]
mod tests;
