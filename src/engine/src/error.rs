//! Engine causes stay here, outside the portable wire/disk contract.
use std::borrow::Cow;
use storage_contract::ErrorCode;

mod published;
pub use published::{
    PublishedRecoveryReconciliation, PublishedWorkspaceReconciliation, RecoveryReconciliationStage,
};

#[derive(Debug, error_definition::Error)]
pub enum Error {
    #[error(transparent)]
    ContainerDrafts(crate::engine::container_drafts::DraftError),
    #[error(transparent)]
    ContainerEdit(storage_contract::container_edit::EditConflict),
    #[error(transparent)]
    ContainerTags(ContainerTagsError),
    #[error(transparent)]
    PathChange(PathChangeError),
    #[error(transparent)]
    VersionNames(crate::container_storage::VersionNameError),
    #[error(transparent)]
    ContainerName(crate::business::namespace::container_name::RenameError),
    #[error(transparent)]
    ConversionNameBudget(crate::directory_conversion::NameBudgetError),
    #[error(transparent)]
    HistoricalBranch(#[from] HistoricalBranchError),
    #[error(transparent)]
    BatchUndo(crate::BatchUndoError),
    #[error("{}: {}", self.code(), self.message())]
    DirectoryLayout(#[source] Box<crate::DirectoryLayoutError>),
    #[error(transparent)]
    DiskObservation(crate::DiskObservationFailure),
    #[error(transparent)]
    PublishedWorkspaceReconciliation(Box<PublishedWorkspaceReconciliation>),
    #[error(transparent)]
    PublishedRecoveryReconciliation(Box<PublishedRecoveryReconciliation>),
    #[error("IO_ERROR: system clock precedes the Unix epoch: {0}")]
    Clock(#[from] std::time::SystemTimeError),
    #[error("IO_ERROR: system clock is outside the UTC timestamp range: {0}")]
    ClockRange(#[source] date_time::error::ComponentRange),
    #[error(transparent)]
    Checkpoint(#[from] CheckpointError),
    #[error(transparent)]
    Contract(#[from] storage_contract::Error),
    #[error("{}: {}", self.code(), self.message())]
    Filesystem(#[from] platform_io::fs_safety::Error),
    #[error("INDEX_REBUILD_FAILED: {0}")]
    Sqlite(#[from] platform_io::sqlite::Error),
    #[error("INDEX_REBUILD_FAILED: {0}")]
    Database(#[from] sqlite_access::Error),
    #[error("CHECKSUM_MISMATCH: {0}")]
    Payload(#[from] platform_io::payload::Error),
    #[error("CONTAINER_LAYOUT_INVALID: {0}")]
    Timestamp(#[source] date_time::error::Parse),
    #[error("CONTAINER_LAYOUT_INVALID: {0}")]
    DateFormat(#[source] date_time::error::Format),
    #[error("HISTORY_PACKAGE_INVALID: {0}")]
    TimestampRange(#[source] date_time::error::ComponentRange),
    #[error("CHECKSUM_MISMATCH: {0}")]
    Digest(#[from] blake3::HexError),
    #[error("CONTAINER_LAYOUT_INVALID: {0}")]
    Integer(#[source] std::num::ParseIntError),
    #[error("PLAN_INVALID: {0}")]
    IntegerRange(#[source] std::num::TryFromIntError),
    #[error("directory name is not UTF-8: {0:?}")]
    PathEncoding(std::ffi::OsString),
    #[error("{operation}; cleanup failed: {cleanup}")]
    Cleanup {
        #[source]
        operation: Box<Error>,
        cleanup: std::io::Error,
    },
    #[error("{code}: {message}")]
    Context {
        code: ErrorCode,
        message: String,
        retryable: bool,
        #[source]
        source: Box<Error>,
    },
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        storage_contract::Error::new(code, message).into()
    }
    pub fn context(self, code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Context {
            code,
            message: message.into(),
            retryable: false,
            source: Box::new(self),
        }
    }
    pub fn retryable(self) -> Self {
        match self {
            published @ (Self::PublishedWorkspaceReconciliation(_)
            | Self::PublishedRecoveryReconciliation(_)) => published,
            Self::Contract(source) => Self::Contract(source.retryable()),
            Self::Cleanup { operation, cleanup } => Self::Cleanup {
                operation: Box::new(operation.retryable()),
                cleanup,
            },
            Self::Context {
                code,
                message,
                source,
                ..
            } => Self::Context {
                code,
                message,
                retryable: true,
                source,
            },
            source @ (Self::HistoricalBranch(_)
            | Self::ContainerTags(_)
            | Self::ContainerEdit(_)
            | Self::ContainerDrafts(_)
            | Self::PathChange(_)
            | Self::VersionNames(_)
            | Self::ContainerName(_)
            | Self::BatchUndo(_)
            | Self::ConversionNameBudget(_)
            | Self::DirectoryLayout(_)
            | Self::DiskObservation(_)
            | Self::Checkpoint(_)
            | Self::Clock(_)
            | Self::ClockRange(_)
            | Self::Filesystem(_)
            | Self::Sqlite(_)
            | Self::Database(_)
            | Self::Payload(_)
            | Self::Timestamp(_)
            | Self::DateFormat(_)
            | Self::TimestampRange(_)
            | Self::Digest(_)
            | Self::Integer(_)
            | Self::IntegerRange(_)
            | Self::PathEncoding(_)) => Self::Context {
                code: source.code(),
                message: source.message().into_owned(),
                retryable: true,
                source: Box::new(source),
            },
        }
    }
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Contract(source) => source.is_retryable(),
            Self::Cleanup { operation, .. } => operation.is_retryable(),
            Self::Context { retryable, .. } => *retryable,
            Self::HistoricalBranch(_)
            | Self::ContainerTags(_)
            | Self::ContainerEdit(_)
            | Self::ContainerDrafts(_)
            | Self::PathChange(_)
            | Self::VersionNames(_)
            | Self::ContainerName(_)
            | Self::BatchUndo(_)
            | Self::ConversionNameBudget(_)
            | Self::DirectoryLayout(_)
            | Self::DiskObservation(_)
            | Self::PublishedWorkspaceReconciliation(_)
            | Self::PublishedRecoveryReconciliation(_)
            | Self::Checkpoint(_)
            | Self::Clock(_)
            | Self::ClockRange(_)
            | Self::Filesystem(_)
            | Self::Sqlite(_)
            | Self::Database(_)
            | Self::Payload(_)
            | Self::Timestamp(_)
            | Self::DateFormat(_)
            | Self::TimestampRange(_)
            | Self::Digest(_)
            | Self::Integer(_)
            | Self::IntegerRange(_)
            | Self::PathEncoding(_) => false,
        }
    }
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::HistoricalBranch(error) => error.code(),
            Self::ContainerTags(error) => error.code(),
            Self::ContainerEdit(error) => error.code(),
            Self::ContainerDrafts(error) => error.code(),
            Self::PathChange(error) => error.code(),
            Self::VersionNames(error) => error.code(),
            Self::ContainerName(error) => error.code(),
            Self::BatchUndo(error) => error.code(),
            Self::ConversionNameBudget(_) => ErrorCode::PlanInvalid,
            Self::DirectoryLayout(error) => match error.as_ref() {
                crate::DirectoryLayoutError::ResourceLimit => ErrorCode::ResourceLimit,
                crate::DirectoryLayoutError::ExtraDirectoryCollision { .. }
                | crate::DirectoryLayoutError::ReservedMetadataCollision { .. } => {
                    ErrorCode::NameCollision
                }
                crate::DirectoryLayoutError::RepositoryMissing { .. }
                | crate::DirectoryLayoutError::RootNotInitialized { .. }
                | crate::DirectoryLayoutError::RepositoryOwnerMissing { .. }
                | crate::DirectoryLayoutError::SnapshotScopeUnproven { .. }
                | crate::DirectoryLayoutError::SnapshotStructureUnproven { .. }
                | crate::DirectoryLayoutError::ExtraDirectoryName(_)
                | crate::DirectoryLayoutError::MainStructureConflict { .. }
                | crate::DirectoryLayoutError::MultipleRepositoryMounts { .. }
                | crate::DirectoryLayoutError::RepositoryCycle { .. }
                | crate::DirectoryLayoutError::UnplacedRepository { .. }
                | crate::DirectoryLayoutError::MultipleMainOccurrences { .. }
                | crate::DirectoryLayoutError::CurrentNamespaceUnproven { .. }
                | crate::DirectoryLayoutError::CurrentMainUnmanagedConflict { .. }
                | crate::DirectoryLayoutError::FileOwnerMissing { .. }
                | crate::DirectoryLayoutError::FileOwnerAmbiguous { .. }
                | crate::DirectoryLayoutError::FileOwnerCrossesRoot { .. }
                | crate::DirectoryLayoutError::UnwrittenFileIdentity { .. }
                | crate::DirectoryLayoutError::VersionName(_)
                | crate::DirectoryLayoutError::PhysicalPath { .. } => {
                    ErrorCode::ContainerLayoutInvalid
                }
            },
            Self::DiskObservation(error) => error.source.code(),
            Self::PublishedWorkspaceReconciliation(_)
            | Self::PublishedRecoveryReconciliation(_) => ErrorCode::RecoveryRequired,
            Self::Contract(source) => source.code(),
            Self::Cleanup { operation, .. } => operation.code(),
            Self::Context { code, .. } => *code,
            Self::Clock(_) | Self::ClockRange(_) => ErrorCode::IoError,
            Self::Checkpoint(_) => ErrorCode::CheckpointInvalid,
            Self::Filesystem(source) => filesystem_code(source),
            Self::Sqlite(_) | Self::Database(_) => ErrorCode::IndexRebuildFailed,
            Self::Payload(_) | Self::Digest(_) => ErrorCode::ChecksumMismatch,
            Self::Timestamp(_) | Self::DateFormat(_) | Self::Integer(_) => {
                ErrorCode::ContainerLayoutInvalid
            }
            Self::IntegerRange(_) => ErrorCode::PlanInvalid,
            Self::TimestampRange(_) => ErrorCode::HistoryPackageInvalid,
            Self::PathEncoding(_) => ErrorCode::UnsafePath,
        }
    }
    pub fn message(&self) -> Cow<'_, str> {
        match self {
            Self::HistoricalBranch(error) => error.to_string().into(),
            Self::ContainerTags(error) => error.to_string().into(),
            Self::ContainerEdit(error) => error.to_string().into(),
            Self::ContainerDrafts(error) => error.to_string().into(),
            Self::PathChange(error) => error.to_string().into(),
            Self::VersionNames(error) => error.to_string().into(),
            Self::ContainerName(error) => error.to_string().into(),
            Self::BatchUndo(error) => error.to_string().into(),
            Self::ConversionNameBudget(error) => error.to_string().into(),
            Self::DirectoryLayout(error) => error.to_string().into(),
            Self::DiskObservation(_) => "disk observation unavailable".into(),
            Self::PublishedWorkspaceReconciliation(error) => error.message().into(),
            Self::PublishedRecoveryReconciliation(error) => error.message().into(),
            Self::Contract(source) => source.message(),
            Self::Cleanup { operation, cleanup } => {
                format!("{}; cleanup failed: {cleanup}", operation.message()).into()
            }
            Self::Context { message, .. } => message.as_str().into(),
            Self::Filesystem(source) => match source {
                platform_io::fs_safety::Error::Io(source)
                | platform_io::fs_safety::Error::PublishedDurabilityUnknown(source) => {
                    source.to_string().into()
                }
                platform_io::fs_safety::Error::DestinationExists(path) => {
                    path.display().to_string().into()
                }
                source @ (platform_io::fs_safety::Error::Cleanup { .. }
                | platform_io::fs_safety::Error::Randomness(_)
                | platform_io::fs_safety::Error::AbsolutePathRequired(_)
                | platform_io::fs_safety::Error::UnsafeRelativePath(_)
                | platform_io::fs_safety::Error::UnsafeFileType(_)
                | platform_io::fs_safety::Error::MultipleLinks(_)
                | platform_io::fs_safety::Error::UnsafePermissions(_)
                | platform_io::fs_safety::Error::IdentityChanged(_)
                | platform_io::fs_safety::Error::AlreadyLocked(_)
                | platform_io::fs_safety::Error::BudgetExceeded) => source.to_string().into(),
            },
            Self::Clock(source) => source.to_string().into(),
            Self::ClockRange(source) => source.to_string().into(),
            Self::Checkpoint(source) => source.to_string().into(),
            Self::Sqlite(source) => source.to_string().into(),
            Self::Database(source) => source.to_string().into(),
            Self::Payload(source) => source.to_string().into(),
            Self::Timestamp(source) => source.to_string().into(),
            Self::DateFormat(source) => source.to_string().into(),
            Self::TimestampRange(source) => source.to_string().into(),
            Self::Digest(source) => source.to_string().into(),
            Self::Integer(source) => source.to_string().into(),
            Self::IntegerRange(source) => source.to_string().into(),
            Self::PathEncoding(path) => format!("directory name is not UTF-8: {path:?}").into(),
        }
    }

    /// Known publication is separate from completion of the local service work.
    /// This receipt is response data, not authorization or permission to unblock writes.
    pub fn published_result(&self) -> Option<&crate::OperationResult> {
        match self {
            Self::PublishedWorkspaceReconciliation(error) => Some(error.result()),
            Self::PublishedRecoveryReconciliation(error) => Some(error.result()),
            Self::Context { source, .. } => source.published_result(),
            Self::Cleanup { operation, .. } => operation.published_result(),
            Self::HistoricalBranch(_)
            | Self::ContainerTags(_)
            | Self::ContainerEdit(_)
            | Self::ContainerDrafts(_)
            | Self::PathChange(_)
            | Self::VersionNames(_)
            | Self::ContainerName(_)
            | Self::BatchUndo(_)
            | Self::ConversionNameBudget(_)
            | Self::DirectoryLayout(_)
            | Self::DiskObservation(_)
            | Self::Clock(_)
            | Self::ClockRange(_)
            | Self::Checkpoint(_)
            | Self::Contract(_)
            | Self::Filesystem(_)
            | Self::Sqlite(_)
            | Self::Database(_)
            | Self::Payload(_)
            | Self::Timestamp(_)
            | Self::DateFormat(_)
            | Self::TimestampRange(_)
            | Self::Digest(_)
            | Self::Integer(_)
            | Self::IntegerRange(_)
            | Self::PathEncoding(_) => None,
        }
    }
}
impl From<crate::DirectoryLayoutError> for Error {
    fn from(source: crate::DirectoryLayoutError) -> Self {
        Self::DirectoryLayout(Box::new(source))
    }
}
impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Self::Contract(source.into())
    }
}
impl From<json_codec::Error> for Error {
    fn from(source: json_codec::Error) -> Self {
        Self::Contract(source.into())
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/error/tests.rs"]
mod tests;

fn filesystem_code(source: &platform_io::fs_safety::Error) -> ErrorCode {
    match source {
        platform_io::fs_safety::Error::Cleanup { operation, .. } => filesystem_code(operation),

        platform_io::fs_safety::Error::Io(source)
            if matches!(source.raw_os_error(), Some(20 | 40)) =>
        {
            ErrorCode::UnsafePath
        }
        platform_io::fs_safety::Error::Io(source) if source.raw_os_error() == Some(28) => {
            ErrorCode::NoSpace
        }
        platform_io::fs_safety::Error::Io(_) => ErrorCode::IoError,
        platform_io::fs_safety::Error::PublishedDurabilityUnknown(_) => {
            ErrorCode::DurabilityUnknown
        }
        platform_io::fs_safety::Error::DestinationExists(_) => ErrorCode::DestinationExists,
        platform_io::fs_safety::Error::Randomness(_)
        | platform_io::fs_safety::Error::AbsolutePathRequired(_)
        | platform_io::fs_safety::Error::UnsafeRelativePath(_)
        | platform_io::fs_safety::Error::UnsafeFileType(_)
        | platform_io::fs_safety::Error::MultipleLinks(_)
        | platform_io::fs_safety::Error::UnsafePermissions(_)
        | platform_io::fs_safety::Error::IdentityChanged(_)
        | platform_io::fs_safety::Error::AlreadyLocked(_)
        | platform_io::fs_safety::Error::BudgetExceeded => ErrorCode::UnsafePath,
    }
}

impl From<name_policy::Error> for Error {
    fn from(source: name_policy::Error) -> Self {
        Self::Contract(source.into())
    }
}

/// Local checkpoint rejection causes; journals remain the recovery authority.
#[derive(Debug, error_definition::Error)]
pub enum CheckpointError {
    #[error("coverage byte budget exceeded ({current} + {added} bytes, limit {limit}) at {path:?}")]
    CoverageBytes {
        path: std::path::PathBuf,
        current: usize,
        added: usize,
        limit: usize,
    },
    #[error("coverage entry budget exceeded (attempted {actual}, limit {limit}) at {path:?}")]
    CoverageEntries {
        path: std::path::PathBuf,
        actual: usize,
        limit: usize,
    },
    #[error("directory entry budget exceeded (attempted {actual}, limit {limit}) at {path:?}")]
    DirectoryEntries {
        path: std::path::PathBuf,
        actual: usize,
        limit: usize,
    },
    #[error("directory name byte budget exceeded (attempted {actual}, limit {limit}) at {path:?}")]
    DirectoryNameBytes {
        path: std::path::PathBuf,
        actual: usize,
        limit: usize,
    },
    #[error("version ID budget exceeded (actual {actual}, limit {limit})")]
    VersionIds { actual: usize, limit: usize },
    #[error("absent path budget exceeded (actual {actual}, limit {limit}) for disk {disk}")]
    AbsentPaths {
        disk: String,
        actual: usize,
        limit: usize,
    },
    #[error("invalid coverage path {0:?}")]
    InvalidPath(std::path::PathBuf),
    #[error("unsupported coverage file type at {0:?}")]
    FileType(std::path::PathBuf),
    #[error("checkpoint coverage members differ from configured disks")]
    Members,
    #[error("covered directory children differ at {0:?}")]
    DirectoryChildren(std::path::PathBuf),
    #[error("coverage exception is not permitted at {0:?}")]
    InvalidException(std::path::PathBuf),
    #[error("covered file length or link count differs at {0:?}")]
    FileMetadata(std::path::PathBuf),
    #[error("covered file content differs at {0:?}")]
    FileContent(std::path::PathBuf),
    #[error("covered path presence or file type differs at {0:?}")]
    PathState(std::path::PathBuf),
    #[error("conflicting checkpoint suffix candidates at sequence {0}")]
    SuffixConflict(u64),
    #[error("checkpoint suffix pool, sequence or previous snapshot differs at sequence {0}")]
    SuffixChain(u64),
}

#[derive(Debug, error_definition::Error)]
pub enum HistoricalBranchError {
    #[error("initialize the directory repository before creating a historical branch")]
    Uninitialized,
    #[error("stage and commit this repository's changes before creating a historical branch")]
    Dirty,
    #[error("historical directory tree {0} is not retained")]
    Missing(storage_contract::DirectoryTreeId),
    #[error("historical directory tree belongs to another repository or stable directory")]
    Scope,
    #[error("historical directory source must name an explicit user commit")]
    Uncommitted,
    #[error("directory branch name already exists")]
    NameCollision,
    #[error("historical directory source includes an unavailable or hidden owned occurrence")]
    Unavailable,
    #[error("historical directory input differs from its source repository")]
    InputScope,
    #[error("decoding historical directory publication input: {0}")]
    Decode(#[source] json_codec::Error),
}

impl HistoricalBranchError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Uninitialized => ErrorCode::DirectoryScopeRequired,
            Self::NameCollision => ErrorCode::NameCollision,
            Self::InputScope | Self::Decode(_) => ErrorCode::PublicationInvalid,
            Self::Dirty
            | Self::Missing(_)
            | Self::Scope
            | Self::Uncommitted
            | Self::Unavailable => ErrorCode::DirectoryBranchConflict,
        }
    }
}

/// Failures at the container-label request or persisted publication boundary.
#[derive(Debug, error_definition::Error)]
pub enum ContainerTagsError {
    #[error("container is not present in the requested writable context")]
    Context,
    #[error("container label generation changed")]
    Conflict,
    #[error("container label publication does not match its request")]
    Publication,
    #[error("container label response does not match its persisted request")]
    Response(#[source] storage_contract::file_container_tags::TagsError),
    #[error("container label publication cannot advance the persisted business generation")]
    PublicationGeneration(#[source] storage_contract::GenerationExhausted),
    #[error("container label generation is exhausted")]
    Exhausted(#[source] storage_contract::GenerationExhausted),
    #[error("container label envelope could not be encoded or decoded")]
    Encoding(#[source] json_codec::Error),
}

impl ContainerTagsError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Context => ErrorCode::FileNotFound,
            Self::Conflict => ErrorCode::HeadConflict,
            Self::Publication
            | Self::Response(_)
            | Self::PublicationGeneration(_)
            | Self::Encoding(_) => ErrorCode::PublicationInvalid,
            Self::Exhausted(_) => ErrorCode::ResourceLimit,
        }
    }
}

/// Failures at the permanent namespace-fact publication boundary.
#[derive(Debug, error_definition::Error)]
pub enum PathChangeError {
    #[error("history package cannot preserve permanent path audit; no package was written")]
    PackageAuditUnsupported,
    #[error("mixed path provenance cannot be copied without its complete source proof")]
    SourceAuditUnsupported,
    #[error("foreign admission cannot preserve path/copy audit; original records were not changed")]
    ForeignSource,
    #[error("file workspace occurrence changed; staged bytes are preserved")]
    WorkspaceLocation,
    #[error("path change does not match its permanent publication")]
    Publication,
    #[error("path change publication exceeds its fact or byte limit")]
    Limit,
    #[error("path change envelope could not be encoded or decoded")]
    Encoding(#[source] json_codec::Error),
    #[error("path change author is missing")]
    Principal(#[source] storage_contract::EmptyPrincipal),
}

impl PathChangeError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Limit => ErrorCode::ResourceLimit,
            Self::WorkspaceLocation => ErrorCode::FileBranchConflict,
            Self::ForeignSource | Self::PackageAuditUnsupported | Self::SourceAuditUnsupported => {
                ErrorCode::UnsupportedOperation
            }
            Self::Publication | Self::Encoding(_) | Self::Principal(_) => ErrorCode::PublicationInvalid,
        }
    }
}
