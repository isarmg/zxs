//! Engine causes stay here, outside the portable wire/disk contract.
use std::borrow::Cow;
use storage_contract::ErrorCode;

#[derive(Debug, error_definition::Error)]
pub enum Error {
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
    Timestamp(#[from] date_time::error::Parse),
    #[error("CONTAINER_LAYOUT_INVALID: {0}")]
    DateFormat(#[from] date_time::error::Format),
    #[error("HISTORY_PACKAGE_INVALID: {0}")]
    TimestampRange(#[from] date_time::error::ComponentRange),
    #[error("CHECKSUM_MISMATCH: {0}")]
    Digest(#[from] blake3::HexError),
    #[error("CONTAINER_LAYOUT_INVALID: {0}")]
    Integer(#[from] std::num::ParseIntError),
    #[error("PLAN_INVALID: {0}")]
    IntegerRange(#[from] std::num::TryFromIntError),
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
            source @ (Self::Checkpoint(_)
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
            Self::Checkpoint(_)
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
            Self::Contract(source) => source.code(),
            Self::Cleanup { operation, .. } => operation.code(),
            Self::Context { code, .. } => *code,
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
