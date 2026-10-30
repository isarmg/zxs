use crate::protocol::{
    FrameStream, MAX_READ_BYTES, OpenResult, PROTOCOL, REQUIRED_CAPABILITIES, ReadResult,
    Request as RawRequest, Response,
};

type Request = RawRequest<storage_contract::OperationId>;
use json_codec::Value;
use std::{
    io,
    os::{
        fd::{IntoRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    path::Path,
    sync::{Arc, Mutex},
};
use storage_contract::ErrorCode;

#[derive(Clone)]
pub struct Client(Arc<Mutex<Option<FrameStream>>>);

enum AdmittedResult {
    Value(Value),
    Open(OpenResult, OwnedFd),
    Read(ReadResult),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteErrorCode {
    Server(ErrorCode),
    RpcOutcomeUnknown,
}
impl std::fmt::Display for RemoteErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Server(code) => code.as_str(),
            Self::RpcOutcomeUnknown => "RPC_OUTCOME_UNKNOWN",
        })
    }
}

#[derive(Debug)]
pub struct RemoteError {
    pub code: RemoteErrorCode,
    pub message: String,
    pub operation_id: Option<String>,
    pub retryable: bool,
    errno: i32,
    source: Option<io::Error>,
}
impl std::fmt::Display for RemoteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)?;
        if let Some(operation) = &self.operation_id {
            write!(formatter, "; operation_id={operation}")?;
        }
        Ok(())
    }
}
impl std::error::Error for RemoteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|source| source as _)
    }
}
fn remote_errno(code: ErrorCode) -> i32 {
    match code {
        ErrorCode::NotFound
        | ErrorCode::EntryNotFound
        | ErrorCode::VersionNotFound
        | ErrorCode::BranchNotFound
        | ErrorCode::RepositoryNotFound => libc::ENOENT,
        ErrorCode::Unauthenticated
        | ErrorCode::Unauthorized
        | ErrorCode::PermissionDenied
        | ErrorCode::Forbidden => libc::EACCES,
        ErrorCode::BranchNotActive => libc::EROFS,
        ErrorCode::FileWriteBusy
        | ErrorCode::BranchWriteBusy
        | ErrorCode::WorkspaceBusy
        | ErrorCode::ContentPinned
        | ErrorCode::CommitPreparationBusy => libc::EBUSY,
        ErrorCode::NameConflict | ErrorCode::NameCollision | ErrorCode::ObjectCollision => {
            libc::EEXIST
        }
        ErrorCode::NoSpace | ErrorCode::QuotaExceeded => libc::ENOSPC,
        ErrorCode::FileTooLarge => libc::EFBIG,
        ErrorCode::ResourceLimitExceeded | ErrorCode::ResourceLimit => libc::EAGAIN,
        ErrorCode::NameTooLong | ErrorCode::PathTooDeep => libc::ENAMETOOLONG,
        ErrorCode::DirectoryNotEmpty => libc::ENOTEMPTY,
        ErrorCode::IsDirectory => libc::EISDIR,
        ErrorCode::ParentNotDirectory | ErrorCode::NotDirectory => libc::ENOTDIR,
        ErrorCode::OperationUnsupported
        | ErrorCode::UnsupportedOperation
        | ErrorCode::FormatUnsupported => libc::ENOTSUP,
        ErrorCode::InvalidRequest
        | ErrorCode::InvalidMove
        | ErrorCode::NameInvalidCharacter
        | ErrorCode::NameReserved
        | ErrorCode::NameNotNormalized
        | ErrorCode::PathInvalid => libc::EINVAL,
        ErrorCode::SmbRebindRequired | ErrorCode::TreeMoved | ErrorCode::BranchArchived => {
            libc::ESTALE
        }
        ErrorCode::FileVersionConflict
        | ErrorCode::FileConflict
        | ErrorCode::FileBranchConflict
        | ErrorCode::DirectoryBranchConflict
        | ErrorCode::DirectoryAliasConflict
        | ErrorCode::BatchMappingConflict
        | ErrorCode::BatchScopeConflict
        | ErrorCode::NamespaceConflict
        | ErrorCode::HeadConflict
        | ErrorCode::CommitPreparationStale
        | ErrorCode::WorkspaceRebaseRequired => libc::EAGAIN,
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
        | ErrorCode::BaseHeadMismatch
        | ErrorCode::BootstrapExpired
        | ErrorCode::CheckpointInvalid
        | ErrorCode::ChecksumFormatInvalid
        | ErrorCode::ChecksumMismatch
        | ErrorCode::CommitInDoubt
        | ErrorCode::CommitNotFound
        | ErrorCode::ConfigConflict
        | ErrorCode::ConfigInvalid
        | ErrorCode::ContainerAmbiguous
        | ErrorCode::ContainerLayoutInvalid
        | ErrorCode::ContentCorrupt
        | ErrorCode::ContentMissing
        | ErrorCode::ContentPurged
        | ErrorCode::ContentUnavailable
        | ErrorCode::CurrentPasswordInvalid
        | ErrorCode::DestinationExists
        | ErrorCode::DirectoryBranchIncomplete
        | ErrorCode::DirectoryBranchNotFound
        | ErrorCode::DirectoryBranchScope
        | ErrorCode::DirectoryHistorySplitUnsupported
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
        | ErrorCode::Fenced
        | ErrorCode::FencingTokenExpired
        | ErrorCode::FencingTokenInvalid
        | ErrorCode::FilesystemRequired
        | ErrorCode::FileBranchNotFound
        | ErrorCode::FileBranchRequired
        | ErrorCode::FileBranchScope
        | ErrorCode::FileBranchSelected
        | ErrorCode::FileBusy
        | ErrorCode::FileHistorySplitUnsupported
        | ErrorCode::FileNodeHistorySplitUnsupported
        | ErrorCode::FileNodeNotFound
        | ErrorCode::FileNotFound
        | ErrorCode::GenerationMismatch
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
        | ErrorCode::InvalidObjectType
        | ErrorCode::InvalidResponse
        | ErrorCode::InvalidSession
        | ErrorCode::InvalidSetupRequest
        | ErrorCode::InvalidSlotId
        | ErrorCode::InvalidTag
        | ErrorCode::InDoubt
        | ErrorCode::IoError
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
        | ErrorCode::NoWindow
        | ErrorCode::ObjectCorrupt
        | ErrorCode::OfflineConfigurationRequired
        | ErrorCode::OperationAborted
        | ErrorCode::OperationIdConflict
        | ErrorCode::OperationNotFound
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
        | ErrorCode::RangeNotSatisfiable
        | ErrorCode::RecoveryPlanNotFound
        | ErrorCode::RecoveryRequired
        | ErrorCode::RecoveryWorkerFailed
        | ErrorCode::ReplicaTargetUnavailable
        | ErrorCode::RequestBindingMismatch
        | ErrorCode::RequestDigestMismatch
        | ErrorCode::RequestTimeout
        | ErrorCode::ResponseTooLarge
        | ErrorCode::RestoreBackupConflict
        | ErrorCode::RouteIncomplete
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
        | ErrorCode::SmbSpoolUnavailable
        | ErrorCode::SmbUnavailable
        | ErrorCode::SourceChanged
        | ErrorCode::SplitInvalid
        | ErrorCode::SplitPlanStale
        | ErrorCode::SplitRootForbidden
        | ErrorCode::StaleUploadGeneration
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
        | ErrorCode::UnsafePath
        | ErrorCode::UnsupportedDeviceTopology
        | ErrorCode::UnsupportedMediaType
        | ErrorCode::UploadGenerationConflict
        | ErrorCode::UploadIncomplete
        | ErrorCode::UploadRangeConflict
        | ErrorCode::UploadStateInvalid
        | ErrorCode::WebAssetsUnavailable
        | ErrorCode::WorkspaceChanged
        | ErrorCode::WorkspaceClosed
        | ErrorCode::WorkspaceDependencyBusy
        | ErrorCode::WorkspaceNotFound
        | ErrorCode::WorkspaceNotSealed
        | ErrorCode::WriterAlreadyActive => libc::EIO,
    }
}
#[derive(Debug)]
struct CloseFailure {
    operation: io::Error,
    close: io::Error,
}
impl std::fmt::Display for CloseFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}; descriptor close also failed: {}",
            self.operation, self.close
        )
    }
}
impl std::error::Error for CloseFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.operation)
    }
}

/// Preserve remote diagnostics while presenting the corresponding filesystem errno.
pub fn error_errno(error: &io::Error) -> i32 {
    error
        .raw_os_error()
        .or_else(|| {
            error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<RemoteError>())
                .map(|remote| remote.errno)
                .or_else(|| {
                    error
                        .get_ref()
                        .and_then(|inner| inner.downcast_ref::<CloseFailure>())
                        .map(|failure| error_errno(&failure.operation))
                })
        })
        .unwrap_or_else(|| match error.kind() {
            io::ErrorKind::InvalidInput => libc::EINVAL,
            io::ErrorKind::PermissionDenied => libc::EACCES,
            io::ErrorKind::OutOfMemory => libc::ENOMEM,
            _ => libc::EIO,
        })
}

/// Finish an already-removed VFS handle and always release its local descriptor.
///
/// Close is terminal even when publication fails. If a worker poisoned the I/O
/// lock, detach instead: neither publishing suspect bytes nor aborting/deleting
/// them is safe. The guard still serializes against outstanding FD users.
pub fn close_descriptor<T>(
    handle: u64,
    descriptor: OwnedFd,
    io_lock: &Mutex<T>,
    finish: impl FnOnce(Request) -> io::Result<()>,
) -> io::Result<()> {
    let (guard, poisoned) = match io_lock.lock() {
        Ok(guard) => (guard, false),
        Err(error) => (error.into_inner(), true),
    };
    let request = if poisoned {
        Request::Detach { handle }
    } else {
        Request::Close { handle }
    };
    let saved = finish(request);
    // SAFETY: into_raw_fd transfers the sole ownership to close. Do not retry a
    // failed close: the numeric descriptor may already have been reused.
    let closed = unsafe { libc::close(descriptor.into_raw_fd()) };
    let close_error = (closed < 0).then(io::Error::last_os_error);
    drop(guard);
    if let Err(operation) = saved {
        return Err(match close_error {
            Some(close) => io::Error::new(operation.kind(), CloseFailure { operation, close }),
            None => operation,
        });
    }
    match close_error {
        Some(error) => Err(error),
        None if poisoned => Err(io::Error::from_raw_os_error(libc::EIO)),
        None => Ok(()),
    }
}

/// Whether a descriptor mutation changed the file and must open a generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mutation {
    Changed,
    Unchanged,
}

/// Write sequencing for one writable remote handle, owned by its I/O lock.
///
/// Every mutation is admitted by `PrepareWrite` before it happens. The server
/// keeps a handle dirty from its first successful `Dirty` until a save path
/// (Flush, write-through Dirty or Close) publishes it, so later changes in the
/// same generation need no further notice. Write-through handles report every
/// change because their `Dirty` performs the save.
#[derive(Debug)]
pub struct WriteSequence {
    write_through: bool,
    generation_announced: bool,
}

impl WriteSequence {
    pub fn new(write_through: bool) -> Self {
        Self {
            write_through,
            generation_announced: false,
        }
    }

    /// Admit, perform and report one descriptor mutation. A failed admission
    /// leaves the descriptor untouched; a failed or unknown `Dirty` leaves the
    /// generation unannounced so the next change reports it again.
    pub fn mutate<T>(
        &mut self,
        handle: u64,
        end: u64,
        bytes: u64,
        mut call: impl FnMut(Request) -> io::Result<Value>,
        mutation: impl FnOnce() -> io::Result<(T, Mutation)>,
    ) -> io::Result<T> {
        call(Request::PrepareWrite { handle, end, bytes })?;
        let (value, outcome) = mutation()?;
        if outcome == Mutation::Changed && (self.write_through || !self.generation_announced) {
            self.generation_announced = false;
            call(Request::Dirty { handle })?;
            self.generation_announced = !self.write_through;
        }
        Ok(value)
    }

    /// Flush may publish and end the generation even when it fails.
    pub fn flush(
        &mut self,
        handle: u64,
        call: impl FnOnce(Request) -> io::Result<Value>,
    ) -> io::Result<Value> {
        self.generation_announced = false;
        call(Request::Flush { handle })
    }
}

impl Client {
    pub fn connect(socket: &Path, username: &str, repository_id: &str) -> io::Result<Self> {
        crate::protocol::validate_username(username)?;
        Self::connect_stream(UnixStream::connect(socket)?, username, repository_id)
    }
    fn connect_stream(stream: UnixStream, username: &str, repository_id: &str) -> io::Result<Self> {
        let client = Self(Arc::new(Mutex::new(Some(FrameStream::new(stream)?))));
        let result = client.call(Request::Hello {
            protocol: PROTOCOL.into(),
            capabilities: REQUIRED_CAPABILITIES
                .iter()
                .map(|value| (*value).into())
                .collect(),
            username: username.into(),
            repository_id: repository_id.into(),
        })?;
        if result.get("protocol").and_then(Value::as_str) != Some(PROTOCOL)
            || !result
                .get("capabilities")
                .and_then(Value::as_array)
                .is_some_and(|capabilities| {
                    REQUIRED_CAPABILITIES.iter().all(|required| {
                        capabilities
                            .iter()
                            .any(|capability| capability.as_str() == Some(required))
                    })
                })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SMB bridge did not negotiate authenticated sessions, durable generation writes and verified range reads",
            ));
        }
        Ok(client)
    }
    /// Exchange an ordinary request; `open` and `read` own their typed admission.
    pub fn call(&self, request: Request) -> io::Result<Value> {
        if matches!(request, Request::Open { .. } | Request::Read { .. }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "descriptor and byte reads require their typed client method",
            ));
        }
        match self.exchange(request)? {
            AdmittedResult::Value(value) => Ok(value),
            AdmittedResult::Open(..) | AdmittedResult::Read(..) => {
                unreachable!("invariant: ordinary requests admit untyped values")
            }
        }
    }
    fn exchange(&self, request: Request) -> io::Result<AdmittedResult> {
        let mut connection = self.0.lock().expect("RPC connection lock poisoned");
        let stream = connection.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "RPC connection is closed")
        })?;
        let exchange = stream
            .send(&request, None)
            .and_then(|()| stream.receive::<Response>())
            .and_then(|(response, fd)| {
                // Descriptor ownership is part of the response contract, not
                // an optional convenience for individual VFS callers. Reject
                // malformed replies while still holding the shared lock so no
                // clone can issue another request on a compromised session.
                let expects_fd = matches!(request, Request::Open { .. })
                    && matches!(response, Response::Ok { .. });
                if fd.is_some() != expects_fd {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "RPC response descriptor does not match the request",
                    ));
                }
                // The outer result admits the frame and descriptor; the inner
                // result preserves a complete remote business error without
                // disconnecting this otherwise valid session.
                Ok(match response {
                    Response::Ok { result } => Ok(if expects_fd {
                        // Decode once while locked. Invalid metadata must close
                        // the session before any clone can use the remote handle.
                        let result: OpenResult = json_codec::from_value(result)
                            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                        AdmittedResult::Open(
                            result,
                            fd.expect("invariant: successful open response has a descriptor"),
                        )
                    } else if let Request::Read { length, .. } = request {
                        let result: ReadResult = json_codec::from_value(result)
                            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                        if result.bytes.len() > length as usize
                            || result.bytes.len() > MAX_READ_BYTES
                        {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "RPC read exceeds requested extent",
                            ));
                        }
                        AdmittedResult::Read(result)
                    } else {
                        AdmittedResult::Value(result)
                    }),
                    Response::Error {
                        code,
                        message,
                        operation_id,
                        retryable,
                    } => Err(RemoteError {
                        code: RemoteErrorCode::Server(code),
                        message,
                        operation_id,
                        retryable,
                        errno: remote_errno(code),
                        source: None,
                    }),
                })
            });
        let response = match exchange {
            Ok(response) => response,
            Err(error) => {
                // A partial request/response or invalid frame loses message
                // alignment. Close the shared connection before unlocking so
                // every clone fails instead of consuming a stale response.
                *connection = None;
                return Err(match request {
                    Request::Namespace { operation_id, .. } => io::Error::new(
                        error.kind(),
                        RemoteError {
                            code: RemoteErrorCode::RpcOutcomeUnknown,
                            message: format!(
                                "{error}; query the original operation before retrying"
                            ),
                            operation_id: Some(operation_id.to_string()),
                            retryable: false,
                            errno: error.raw_os_error().unwrap_or(libc::EIO),
                            source: Some(error),
                        },
                    ),
                    _ => error,
                });
            }
        };
        response.map_err(|error| {
            io::Error::new(io::Error::from_raw_os_error(error.errno).kind(), error)
        })
    }
    /// Open a remote handle and transfer its admitted metadata and owned FD.
    /// Non-Open requests fail before sending; malformed replies close all clones.
    pub fn open(&self, request: Request) -> io::Result<(OpenResult, OwnedFd)> {
        if !matches!(request, Request::Open { .. }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected open request",
            ));
        }
        match self.exchange(request)? {
            AdmittedResult::Open(result, descriptor) => Ok((result, descriptor)),
            AdmittedResult::Value(_) | AdmittedResult::Read(_) => {
                unreachable!("invariant: successful open requests admit typed metadata and an FD")
            }
        }
    }

    /// Fill a host-owned read buffer using bounded verified replies. A short
    /// positive reply can be a checksum-chunk boundary; only zero denotes EOF.
    /// Revocation, unavailable bytes, transport and malformed replies fail the
    /// read. On failure the buffer can contain a verified prefix, which callers
    /// must not report as a successful complete read.
    pub fn read(&self, handle: u64, mut offset: u64, output: &mut [u8]) -> io::Result<usize> {
        let mut copied = 0;
        loop {
            let length = (output.len() - copied).min(MAX_READ_BYTES) as u32;
            let AdmittedResult::Read(result) = self.exchange(Request::Read {
                handle,
                offset,
                length,
            })?
            else {
                unreachable!("invariant: successful reads admit typed bounded bytes")
            };
            let count = result.bytes.len();
            output[copied..copied + count].copy_from_slice(&result.bytes);
            copied += count;
            if count == 0 || copied == output.len() {
                return Ok(copied);
            }
            offset = offset.checked_add(count as u64).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "read offset overflow")
            })?;
        }
    }
}

#[cfg(test)]
#[path = "../../../test/smb/unit/client/tests.rs"]
mod tests;
