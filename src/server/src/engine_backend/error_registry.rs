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
pub(super) const PUBLIC_STATUS: &[(ErrorCode, ::http::StatusCode)] = &[
    (ErrorCode::Unauthorized, ::http::StatusCode::UNAUTHORIZED),
    (ErrorCode::Forbidden, ::http::StatusCode::FORBIDDEN),
    (ErrorCode::ContentPurged, ::http::StatusCode::GONE),
    (ErrorCode::NoSpace, ::http::StatusCode::INSUFFICIENT_STORAGE),
    (
        ErrorCode::ResourceLimitExceeded,
        ::http::StatusCode::TOO_MANY_REQUESTS,
    ),
    (
        ErrorCode::QuotaExceeded,
        ::http::StatusCode::TOO_MANY_REQUESTS,
    ),
    (
        ErrorCode::ContentUnavailable,
        ::http::StatusCode::SERVICE_UNAVAILABLE,
    ),
    (
        ErrorCode::MetadataUnavailable,
        ::http::StatusCode::SERVICE_UNAVAILABLE,
    ),
    (
        ErrorCode::RecoveryRequired,
        ::http::StatusCode::SERVICE_UNAVAILABLE,
    ),
    (
        ErrorCode::ReplicaTargetUnavailable,
        ::http::StatusCode::SERVICE_UNAVAILABLE,
    ),
    (
        ErrorCode::RouteTargetUnavailable,
        ::http::StatusCode::SERVICE_UNAVAILABLE,
    ),
    (
        ErrorCode::CommitInDoubt,
        ::http::StatusCode::SERVICE_UNAVAILABLE,
    ),
    (
        ErrorCode::BackupRouteIncomplete,
        ::http::StatusCode::SERVICE_UNAVAILABLE,
    ),
    (
        ErrorCode::CommitPreparationBusy,
        ::http::StatusCode::CONFLICT,
    ),
    (
        ErrorCode::CommitPreparationStale,
        ::http::StatusCode::CONFLICT,
    ),
    (ErrorCode::PreparationStale, ::http::StatusCode::CONFLICT),
    (ErrorCode::HeadConflict, ::http::StatusCode::CONFLICT),
    (ErrorCode::HistoryChanged, ::http::StatusCode::CONFLICT),
    (ErrorCode::BranchNotActive, ::http::StatusCode::CONFLICT),
    (ErrorCode::Fenced, ::http::StatusCode::CONFLICT),
    (ErrorCode::LeaseExpired, ::http::StatusCode::CONFLICT),
    (
        ErrorCode::UploadGenerationConflict,
        ::http::StatusCode::CONFLICT,
    ),
    (ErrorCode::FileConflict, ::http::StatusCode::CONFLICT),
    (ErrorCode::FileBranchConflict, ::http::StatusCode::CONFLICT),
    (
        ErrorCode::DirectoryBranchConflict,
        ::http::StatusCode::CONFLICT,
    ),
    (
        ErrorCode::DirectoryAliasConflict,
        ::http::StatusCode::CONFLICT,
    ),
    (
        ErrorCode::BatchMappingConflict,
        ::http::StatusCode::CONFLICT,
    ),
    (ErrorCode::BatchScopeConflict, ::http::StatusCode::CONFLICT),
    (
        ErrorCode::IdempotencyKeyReused,
        ::http::StatusCode::CONFLICT,
    ),
    (ErrorCode::SplitPlanStale, ::http::StatusCode::CONFLICT),
    (
        ErrorCode::PurgeBlockedByReference,
        ::http::StatusCode::CONFLICT,
    ),
    (ErrorCode::ContentPinned, ::http::StatusCode::CONFLICT),
    (ErrorCode::ConfigConflict, ::http::StatusCode::CONFLICT),
    (ErrorCode::BranchWriteBusy, ::http::StatusCode::CONFLICT),
    (ErrorCode::FileWriteBusy, ::http::StatusCode::CONFLICT),
    (ErrorCode::NamespaceConflict, ::http::StatusCode::CONFLICT),
    (ErrorCode::NameCollision, ::http::StatusCode::CONFLICT),
    (ErrorCode::WorkspaceClosed, ::http::StatusCode::CONFLICT),
    (ErrorCode::PlanStale, ::http::StatusCode::CONFLICT),
    (ErrorCode::PlanClosed, ::http::StatusCode::CONFLICT),
    (ErrorCode::TreeMoved, ::http::StatusCode::CONFLICT),
    (
        ErrorCode::RestoreBackupConflict,
        ::http::StatusCode::CONFLICT,
    ),
    (
        ErrorCode::DirectoryBranchIncomplete,
        ::http::StatusCode::CONFLICT,
    ),
    (
        ErrorCode::DirectoryScopeRequired,
        ::http::StatusCode::CONFLICT,
    ),
    (
        ErrorCode::DirectoryHistorySplitUnsupported,
        ::http::StatusCode::BAD_REQUEST,
    ),
    (
        ErrorCode::FileNodeHistorySplitUnsupported,
        ::http::StatusCode::BAD_REQUEST,
    ),
    (
        ErrorCode::IoError,
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
    ),
    (
        ErrorCode::DurabilityUnknown,
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
    ),
    (
        ErrorCode::StateInvalid,
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
    ),
    (
        ErrorCode::InternalError,
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
    ),
    (
        ErrorCode::ObjectCorrupt,
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
    ),
    (
        ErrorCode::LogCorrupt,
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
    ),
    (
        ErrorCode::PublicationInvalid,
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
    ),
    (
        ErrorCode::OperationUnsupported,
        ::http::StatusCode::NOT_IMPLEMENTED,
    ),
    (
        ErrorCode::PlanUnsupported,
        ::http::StatusCode::NOT_IMPLEMENTED,
    ),
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
        | ErrorCode::HistoryChanged
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

pub(super) fn status(code: ErrorCode) -> ::http::StatusCode {
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
        | ErrorCode::WriterAlreadyActive => ::http::StatusCode::BAD_REQUEST,
        ErrorCode::Unauthorized => ::http::StatusCode::UNAUTHORIZED,
        ErrorCode::Forbidden => ::http::StatusCode::FORBIDDEN,
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
        | ErrorCode::WorkspaceNotFound => ::http::StatusCode::NOT_FOUND,
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
        | ErrorCode::HistoryChanged
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
        | ErrorCode::WorkspaceClosed => ::http::StatusCode::CONFLICT,
        ErrorCode::ContentPurged => ::http::StatusCode::GONE,
        ErrorCode::QuotaExceeded | ErrorCode::ResourceLimitExceeded => {
            ::http::StatusCode::TOO_MANY_REQUESTS
        }
        ErrorCode::DurabilityUnknown
        | ErrorCode::InternalError
        | ErrorCode::IoError
        | ErrorCode::LogCorrupt
        | ErrorCode::ObjectCorrupt
        | ErrorCode::PublicationInvalid
        | ErrorCode::StateInvalid => ::http::StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::OperationUnsupported | ErrorCode::PlanUnsupported => {
            ::http::StatusCode::NOT_IMPLEMENTED
        }
        ErrorCode::BackupRouteIncomplete
        | ErrorCode::CommitInDoubt
        | ErrorCode::ContentUnavailable
        | ErrorCode::MetadataUnavailable
        | ErrorCode::RecoveryRequired
        | ErrorCode::ReplicaTargetUnavailable
        | ErrorCode::RouteTargetUnavailable => ::http::StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::NoSpace => ::http::StatusCode::INSUFFICIENT_STORAGE,
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/error_registry.rs"]
mod tests;
