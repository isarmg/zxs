//! Closed first-party error identities; strings exist only at wire/display boundaries.
use data_serialization::{Deserialize, Serialize};

macro_rules! error_codes {
    ($($variant:ident => $wire:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
        #[serialization(rename_all = "SCREAMING_SNAKE_CASE")]
        pub enum ErrorCode { $($variant),+ }
        impl<'de> Deserialize<'de> for ErrorCode {
            fn deserialize<D: data_serialization::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
                struct CodeVisitor;
                impl data_serialization::de::Visitor<'_> for CodeVisitor {
                    type Value = ErrorCode;
                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str("a first-party error code string")
                    }
                    fn visit_str<E: data_serialization::de::Error>(self, token: &str) -> std::result::Result<Self::Value, E> {
                        match token {
                            $($wire => Ok(ErrorCode::$variant),)+
                            _ => Err(E::unknown_variant(token, &[$($wire),+])),
                        }
                    }
                }
                deserializer.deserialize_str(CodeVisitor)
            }
        }
        impl ErrorCode {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }
        }
    };
}
error_codes! {
    AlreadyInitialized => "ALREADY_INITIALIZED",
    AuthorityRecoveryRequired => "AUTHORITY_RECOVERY_REQUIRED",
    AuthAlreadyInitialized => "AUTH_ALREADY_INITIALIZED",
    AuthRecoveryRequired => "AUTH_RECOVERY_REQUIRED",
    AuthStateInvalid => "AUTH_STATE_INVALID",
    AuthStoreBusy => "AUTH_STORE_BUSY",
    BackupConfigurationRequired => "BACKUP_CONFIGURATION_REQUIRED",
    BackupPathConflict => "BACKUP_PATH_CONFLICT",
    BackupPrefixRequired => "BACKUP_PREFIX_REQUIRED",
    BackupRouteIncomplete => "BACKUP_ROUTE_INCOMPLETE",
    BaseHeadMismatch => "BASE_HEAD_MISMATCH",
    BatchMappingConflict => "BATCH_MAPPING_CONFLICT",
    BatchScopeConflict => "BATCH_SCOPE_CONFLICT",
    BootstrapExpired => "BOOTSTRAP_EXPIRED",
    BranchArchived => "BRANCH_ARCHIVED",
    BranchNotActive => "BRANCH_NOT_ACTIVE",
    BranchNotFound => "BRANCH_NOT_FOUND",
    BranchWriteBusy => "BRANCH_WRITE_BUSY",
    CheckpointInvalid => "CHECKPOINT_INVALID",
    ChecksumFormatInvalid => "CHECKSUM_FORMAT_INVALID",
    ChecksumMismatch => "CHECKSUM_MISMATCH",
    CommitInDoubt => "COMMIT_IN_DOUBT",
    CommitNotFound => "COMMIT_NOT_FOUND",
    CommitPreparationBusy => "COMMIT_PREPARATION_BUSY",
    CommitPreparationStale => "COMMIT_PREPARATION_STALE",
    ConfigConflict => "CONFIG_CONFLICT",
    ConfigInvalid => "CONFIG_INVALID",
    ContainerAmbiguous => "CONTAINER_AMBIGUOUS",
    ContainerLayoutInvalid => "CONTAINER_LAYOUT_INVALID",
    ContentCorrupt => "CONTENT_CORRUPT",
    ContentMissing => "CONTENT_MISSING",
    ContentPinned => "CONTENT_PINNED",
    ContentPurged => "CONTENT_PURGED",
    ContentUnavailable => "CONTENT_UNAVAILABLE",
    CurrentPasswordInvalid => "CURRENT_PASSWORD_INVALID",
    DestinationExists => "DESTINATION_EXISTS",
    DirectoryAliasConflict => "DIRECTORY_ALIAS_CONFLICT",
    DirectoryBranchConflict => "DIRECTORY_BRANCH_CONFLICT",
    DirectoryBranchIncomplete => "DIRECTORY_BRANCH_INCOMPLETE",
    DirectoryBranchNotFound => "DIRECTORY_BRANCH_NOT_FOUND",
    DirectoryBranchScope => "DIRECTORY_BRANCH_SCOPE",
    DirectoryHistorySplitUnsupported => "DIRECTORY_HISTORY_SPLIT_UNSUPPORTED",
    DirectoryNotEmpty => "DIRECTORY_NOT_EMPTY",
    DirectoryNotFound => "DIRECTORY_NOT_FOUND",
    DirectoryScopeRequired => "DIRECTORY_SCOPE_REQUIRED",
    DiskAlreadyEnrolled => "DISK_ALREADY_ENROLLED",
    DiskChanged => "DISK_CHANGED",
    DiskIdentityMismatch => "DISK_IDENTITY_MISMATCH",
    DiskNotFound => "DISK_NOT_FOUND",
    DiskNotIndependent => "DISK_NOT_INDEPENDENT",
    DiskNotMounted => "DISK_NOT_MOUNTED",
    DiskReadOnly => "DISK_READ_ONLY",
    DiskSampleUnavailable => "DISK_SAMPLE_UNAVAILABLE",
    DiskUnavailable => "DISK_UNAVAILABLE",
    DurabilityUnknown => "DURABILITY_UNKNOWN",
    EntryNotFound => "ENTRY_NOT_FOUND",
    Fenced => "FENCED",
    FencingTokenExpired => "FENCING_TOKEN_EXPIRED",
    FencingTokenInvalid => "FENCING_TOKEN_INVALID",
    FilesystemRequired => "FILESYSTEM_REQUIRED",
    FileBranchConflict => "FILE_BRANCH_CONFLICT",
    FileBranchNotFound => "FILE_BRANCH_NOT_FOUND",
    FileBranchRequired => "FILE_BRANCH_REQUIRED",
    FileBranchScope => "FILE_BRANCH_SCOPE",
    FileBranchSelected => "FILE_BRANCH_SELECTED",
    FileBusy => "FILE_BUSY",
    FileConflict => "FILE_CONFLICT",
    FileHistorySplitUnsupported => "FILE_HISTORY_SPLIT_UNSUPPORTED",
    FileNodeHistorySplitUnsupported => "FILE_NODE_HISTORY_SPLIT_UNSUPPORTED",
    FileNodeNotFound => "FILE_NODE_NOT_FOUND",
    FileNotFound => "FILE_NOT_FOUND",
    FileTooLarge => "FILE_TOO_LARGE",
    FileVersionConflict => "FILE_VERSION_CONFLICT",
    FileWriteBusy => "FILE_WRITE_BUSY",
    Forbidden => "FORBIDDEN",
    FormatUnsupported => "FORMAT_UNSUPPORTED",
    GenerationMismatch => "GENERATION_MISMATCH",
    HeadConflict => "HEAD_CONFLICT",
    HistoricalSaveRequiresBranch => "HISTORICAL_SAVE_REQUIRES_BRANCH",
    HistoryChanged => "HISTORY_CHANGED",
    HistoryMismatch => "HISTORY_MISMATCH",
    HistoryNodeNotFound => "HISTORY_NODE_NOT_FOUND",
    HistoryNotFound => "HISTORY_NOT_FOUND",
    HistoryPackageInvalid => "HISTORY_PACKAGE_INVALID",
    HttpError => "HTTP_ERROR",
    HttpUploadsPending => "HTTP_UPLOADS_PENDING",
    HttpUploadsUnavailable => "HTTP_UPLOADS_UNAVAILABLE",
    IdempotencyKeyReused => "IDEMPOTENCY_KEY_REUSED",
    IndexRebuildFailed => "INDEX_REBUILD_FAILED",
    InjectedFailure => "INJECTED_FAILURE",
    InternalError => "INTERNAL_ERROR",
    InvalidAdminResponse => "INVALID_ADMIN_RESPONSE",
    InvalidApiPath => "INVALID_API_PATH",
    InvalidBackupPrefix => "INVALID_BACKUP_PREFIX",
    InvalidCursor => "INVALID_CURSOR",
    InvalidDirectoryPage => "INVALID_DIRECTORY_PAGE",
    InvalidDiskList => "INVALID_DISK_LIST",
    InvalidId => "INVALID_ID",
    InvalidInteger => "INVALID_INTEGER",
    InvalidJson => "INVALID_JSON",
    InvalidMove => "INVALID_MOVE",
    InvalidObjectType => "INVALID_OBJECT_TYPE",
    InvalidRequest => "INVALID_REQUEST",
    InvalidResponse => "INVALID_RESPONSE",
    InvalidSession => "INVALID_SESSION",
    InvalidSetupRequest => "INVALID_SETUP_REQUEST",
    InvalidSlotId => "INVALID_SLOT_ID",
    InvalidTag => "INVALID_TAG",
    InDoubt => "IN_DOUBT",
    IoError => "IO_ERROR",
    IsDirectory => "IS_DIRECTORY",
    ItemNotFound => "ITEM_NOT_FOUND",
    ItemPathMismatch => "ITEM_PATH_MISMATCH",
    LeaseExpired => "LEASE_EXPIRED",
    ListingChanged => "LISTING_CHANGED",
    LogCorrupt => "LOG_CORRUPT",
    LogFork => "LOG_FORK",
    LogFormatUnsupported => "LOG_FORMAT_UNSUPPORTED",
    MainBranchRequired => "MAIN_BRANCH_REQUIRED",
    MergeEndpointMismatch => "MERGE_ENDPOINT_MISMATCH",
    MetadataQuorumUnavailable => "METADATA_QUORUM_UNAVAILABLE",
    MetadataTooLong => "METADATA_TOO_LONG",
    MetadataUnavailable => "METADATA_UNAVAILABLE",
    MethodNotAllowed => "METHOD_NOT_ALLOWED",
    MonitorUnavailable => "MONITOR_UNAVAILABLE",
    MultiDeviceFilesystem => "MULTI_DEVICE_FILESYSTEM",
    NamespaceConflict => "NAMESPACE_CONFLICT",
    NameCollision => "NAME_COLLISION",
    NameConflict => "NAME_CONFLICT",
    NameInvalidCharacter => "NAME_INVALID_CHARACTER",
    NameNotNormalized => "NAME_NOT_NORMALIZED",
    NameReserved => "NAME_RESERVED",
    NameTooLong => "NAME_TOO_LONG",
    NotDirectory => "NOT_DIRECTORY",
    NotFound => "NOT_FOUND",
    NoSpace => "NO_SPACE",
    NoWindow => "NO_WINDOW",
    ObjectCollision => "OBJECT_COLLISION",
    ObjectCorrupt => "OBJECT_CORRUPT",
    OfflineConfigurationRequired => "OFFLINE_CONFIGURATION_REQUIRED",
    OperationAborted => "OPERATION_ABORTED",
    OperationIdConflict => "OPERATION_ID_CONFLICT",
    OperationNotFound => "OPERATION_NOT_FOUND",
    OperationUnsupported => "OPERATION_UNSUPPORTED",
    ParentNotDirectory => "PARENT_NOT_DIRECTORY",
    PathInvalid => "PATH_INVALID",
    PathTooDeep => "PATH_TOO_DEEP",
    PayloadTooLarge => "PAYLOAD_TOO_LARGE",
    PermanentLossNotAcknowledged => "PERMANENT_LOSS_NOT_ACKNOWLEDGED",
    PermissionDenied => "PERMISSION_DENIED",
    PlacementInvalid => "PLACEMENT_INVALID",
    PlacementNotFound => "PLACEMENT_NOT_FOUND",
    PlanClosed => "PLAN_CLOSED",
    PlanInvalid => "PLAN_INVALID",
    PlanNotFound => "PLAN_NOT_FOUND",
    PlanStale => "PLAN_STALE",
    PlanUnsupported => "PLAN_UNSUPPORTED",
    PreparationStale => "PREPARATION_STALE",
    PublicationInvalid => "PUBLICATION_INVALID",
    PurgeBlockedByReference => "PURGE_BLOCKED_BY_REFERENCE",
    QuotaExceeded => "QUOTA_EXCEEDED",
    RangeNotSatisfiable => "RANGE_NOT_SATISFIABLE",
    RecoveryPlanNotFound => "RECOVERY_PLAN_NOT_FOUND",
    RecoveryRequired => "RECOVERY_REQUIRED",
    RecoveryWorkerFailed => "RECOVERY_WORKER_FAILED",
    ReplicaTargetUnavailable => "REPLICA_TARGET_UNAVAILABLE",
    RepositoryNotFound => "REPOSITORY_NOT_FOUND",
    RequestBindingMismatch => "REQUEST_BINDING_MISMATCH",
    RequestDigestMismatch => "REQUEST_DIGEST_MISMATCH",
    RequestTimeout => "REQUEST_TIMEOUT",
    ResourceLimit => "RESOURCE_LIMIT",
    ResourceLimitExceeded => "RESOURCE_LIMIT_EXCEEDED",
    ResponseTooLarge => "RESPONSE_TOO_LARGE",
    RestoreBackupConflict => "RESTORE_BACKUP_CONFLICT",
    RouteIncomplete => "ROUTE_INCOMPLETE",
    RouteTargetUnavailable => "ROUTE_TARGET_UNAVAILABLE",
    SaveModeUnsupported => "SAVE_MODE_UNSUPPORTED",
    ScopeChanged => "SCOPE_CHANGED",
    SealedManifestMismatch => "SEALED_MANIFEST_MISMATCH",
    ServiceStopping => "SERVICE_STOPPING",
    SessionChanged => "SESSION_CHANGED",
    SetupBusy => "SETUP_BUSY",
    SetupContentChanged => "SETUP_CONTENT_CHANGED",
    SetupPresetChanged => "SETUP_PRESET_CHANGED",
    SetupProofInvalid => "SETUP_PROOF_INVALID",
    SlotNotFound => "SLOT_NOT_FOUND",
    SmbRebindRequired => "SMB_REBIND_REQUIRED",
    SmbSpoolUnavailable => "SMB_SPOOL_UNAVAILABLE",
    SmbUnavailable => "SMB_UNAVAILABLE",
    SourceChanged => "SOURCE_CHANGED",
    SplitInvalid => "SPLIT_INVALID",
    SplitPlanStale => "SPLIT_PLAN_STALE",
    SplitRootForbidden => "SPLIT_ROOT_FORBIDDEN",
    StaleUploadGeneration => "STALE_UPLOAD_GENERATION",
    StateInvalid => "STATE_INVALID",
    StorageActive => "STORAGE_ACTIVE",
    StorageDraining => "STORAGE_DRAINING",
    StorageRetryRunning => "STORAGE_RETRY_RUNNING",
    StorageUnavailable => "STORAGE_UNAVAILABLE",
    SystemAuthRequired => "SYSTEM_AUTH_REQUIRED",
    SystemRootChanged => "SYSTEM_ROOT_CHANGED",
    TagLimitExceeded => "TAG_LIMIT_EXCEEDED",
    TombstoneInvalid => "TOMBSTONE_INVALID",
    TopologyGenerationMismatch => "TOPOLOGY_GENERATION_MISMATCH",
    TopologyInvalid => "TOPOLOGY_INVALID",
    TransferTimeout => "TRANSFER_TIMEOUT",
    TransportError => "TRANSPORT_ERROR",
    TreeMoved => "TREE_MOVED",
    Unauthenticated => "UNAUTHENTICATED",
    Unauthorized => "UNAUTHORIZED",
    UnsafePath => "UNSAFE_PATH",
    UnsupportedDeviceTopology => "UNSUPPORTED_DEVICE_TOPOLOGY",
    UnsupportedMediaType => "UNSUPPORTED_MEDIA_TYPE",
    UnsupportedOperation => "UNSUPPORTED_OPERATION",
    UploadGenerationConflict => "UPLOAD_GENERATION_CONFLICT",
    UploadIncomplete => "UPLOAD_INCOMPLETE",
    UploadRangeConflict => "UPLOAD_RANGE_CONFLICT",
    UploadStateInvalid => "UPLOAD_STATE_INVALID",
    VersionNotFound => "VERSION_NOT_FOUND",
    WebAssetsUnavailable => "WEB_ASSETS_UNAVAILABLE",
    WorkspaceBusy => "WORKSPACE_BUSY",
    WorkspaceChanged => "WORKSPACE_CHANGED",
    WorkspaceClosed => "WORKSPACE_CLOSED",
    WorkspaceDependencyBusy => "WORKSPACE_DEPENDENCY_BUSY",
    WorkspaceNotFound => "WORKSPACE_NOT_FOUND",
    WorkspaceNotSealed => "WORKSPACE_NOT_SEALED",
    WorkspaceRebaseRequired => "WORKSPACE_REBASE_REQUIRED",
    WriterAlreadyActive => "WRITER_ALREADY_ACTIVE",
}
impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, error_definition::Error)]
pub enum Error {
    #[error("{}: {source}", ErrorCode::from(source))]
    Name {
        #[source]
        source: name_policy::Error,
        retryable: bool,
    },
    #[error("{code}: {message}")]
    Message {
        code: ErrorCode,
        message: String,
        retryable: bool,
    },
    #[error("{}: {source}", io_code(.source))]
    Io {
        #[source]
        source: std::io::Error,
        retryable: bool,
    },
    #[error("INVALID_JSON: {source}")]
    Json {
        #[source]
        source: json_codec::Error,
        retryable: bool,
    },
    #[error("INVALID_INTEGER: unsigned decimal exceeds u64")]
    Integer {
        #[source]
        source: std::num::ParseIntError,
        retryable: bool,
    },
}
fn io_code(source: &std::io::Error) -> ErrorCode {
    if source.raw_os_error() == Some(28) {
        ErrorCode::NoSpace
    } else {
        ErrorCode::IoError
    }
}
impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Message {
            code,
            message: message.into(),
            retryable: false,
        }
    }
    pub fn retryable(mut self) -> Self {
        match &mut self {
            Self::Name { retryable, .. }
            | Self::Message { retryable, .. }
            | Self::Io { retryable, .. }
            | Self::Json { retryable, .. }
            | Self::Integer { retryable, .. } => *retryable = true,
        }
        self
    }
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Name { retryable, .. }
            | Self::Message { retryable, .. }
            | Self::Io { retryable, .. }
            | Self::Json { retryable, .. }
            | Self::Integer { retryable, .. } => *retryable,
        }
    }
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Message { code, .. } => *code,
            Self::Name { source, .. } => source.into(),
            Self::Io { source, .. } => io_code(source),
            Self::Json { .. } => ErrorCode::InvalidJson,
            Self::Integer { .. } => ErrorCode::InvalidInteger,
        }
    }
    pub fn message(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Message { message, .. } => message.as_str().into(),
            Self::Name { source, .. } => source.to_string().into(),
            Self::Io { source, .. } => source.to_string().into(),
            Self::Json { source, .. } => source.to_string().into(),
            Self::Integer { .. } => "unsigned decimal exceeds u64".into(),
        }
    }
}
impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Self::Io {
            source,
            retryable: false,
        }
    }
}
impl From<json_codec::Error> for Error {
    fn from(source: json_codec::Error) -> Self {
        Self::Json {
            source,
            retryable: false,
        }
    }
}
impl From<std::num::ParseIntError> for Error {
    fn from(source: std::num::ParseIntError) -> Self {
        Self::Integer {
            source,
            retryable: false,
        }
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/error/tests.rs"]
mod tests;

impl From<&name_policy::Error> for ErrorCode {
    fn from(error: &name_policy::Error) -> Self {
        match error {
            name_policy::Error::EmptyOrDot
            | name_policy::Error::NameEdge
            | name_policy::Error::UnsupportedCharacter(_) => Self::NameInvalidCharacter,
            name_policy::Error::NameTooLong => Self::NameTooLong,
            name_policy::Error::ReservedName => Self::NameReserved,
            name_policy::Error::NotNormalized(_) => Self::NameNotNormalized,
            name_policy::Error::InvalidPath => Self::PathInvalid,
            name_policy::Error::PathTooDeep => Self::PathTooDeep,
            name_policy::Error::InvalidSlot => Self::InvalidSlotId,
        }
    }
}
impl From<name_policy::Error> for Error {
    fn from(source: name_policy::Error) -> Self {
        Self::Name {
            source,
            retryable: false,
        }
    }
}
