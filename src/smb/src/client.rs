use crate::protocol::{
    FrameStream, OpenResult, PROTOCOL, REQUIRED_CAPABILITIES, Request, Response,
};
use json_codec::Value;
use std::{
    io,
    net::Shutdown,
    os::{
        fd::{IntoRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct Client(Arc<Mutex<Option<FrameStream>>>);

#[derive(Debug)]
pub struct RemoteError {
    pub code: String,
    pub message: String,
    pub operation_id: Option<String>,
    pub retryable: bool,
    errno: i32,
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
impl std::error::Error for RemoteError {}
fn remote_errno(code: &str) -> i32 {
    match code {
        "NOT_FOUND"
        | "ENTRY_NOT_FOUND"
        | "VERSION_NOT_FOUND"
        | "BRANCH_NOT_FOUND"
        | "REPOSITORY_NOT_FOUND" => libc::ENOENT,
        "UNAUTHENTICATED" | "UNAUTHORIZED" | "PERMISSION_DENIED" | "FORBIDDEN" => libc::EACCES,
        "BRANCH_NOT_ACTIVE" | "FILE_BRANCH_READ_ONLY" => libc::EROFS,
        "FILE_WRITE_BUSY"
        | "BRANCH_WRITE_BUSY"
        | "WORKSPACE_BUSY"
        | "CONTENT_PINNED"
        | "COMMIT_PREPARATION_BUSY" => libc::EBUSY,
        "NAME_CONFLICT" | "NAME_COLLISION" | "OBJECT_COLLISION" => libc::EEXIST,
        "NO_SPACE" | "QUOTA_EXCEEDED" => libc::ENOSPC,
        "FILE_TOO_LARGE" => libc::EFBIG,
        "RESOURCE_LIMIT_EXCEEDED" | "RESOURCE_LIMIT" => libc::EAGAIN,
        "NAME_TOO_LONG" | "PATH_TOO_DEEP" => libc::ENAMETOOLONG,
        "DIRECTORY_NOT_EMPTY" => libc::ENOTEMPTY,
        "IS_DIRECTORY" => libc::EISDIR,
        "PARENT_NOT_DIRECTORY" | "NOT_DIRECTORY" => libc::ENOTDIR,
        "UNSUPPORTED"
        | "OPERATION_UNSUPPORTED"
        | "UNSUPPORTED_OPERATION"
        | "FORMAT_UNSUPPORTED" => libc::ENOTSUP,
        "INVALID_REQUEST"
        | "INVALID_NAME"
        | "INVALID_PATH"
        | "INVALID_MOVE"
        | "NAME_INVALID"
        | "NAME_INVALID_CHARACTER"
        | "NAME_RESERVED"
        | "NAME_NOT_NORMALIZED"
        | "PATH_INVALID" => libc::EINVAL,
        "SMB_REBIND_REQUIRED" | "TREE_MOVED" | "BRANCH_ARCHIVED" => libc::ESTALE,
        "FILE_VERSION_CONFLICT"
        | "FILE_CONFLICT"
        | "FILE_BRANCH_CONFLICT"
        | "DIRECTORY_BRANCH_CONFLICT"
        | "DIRECTORY_ALIAS_CONFLICT"
        | "BATCH_MAPPING_CONFLICT"
        | "BATCH_SCOPE_CONFLICT"
        | "NAMESPACE_CONFLICT"
        | "HEAD_CONFLICT"
        | "COMMIT_PREPARATION_STALE"
        | "WORKSPACE_REBASE_REQUIRED" => libc::EAGAIN,
        _ => libc::EIO,
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
        })
        .unwrap_or(libc::EIO)
}

/// Finish an already-removed VFS handle and always release its local descriptor.
///
/// Close is terminal even when publication fails. If a worker poisoned the I/O
/// lock, detach instead: neither publishing suspect bytes nor aborting/deleting
/// them is safe. The guard still serializes against outstanding FD users.
pub fn close_descriptor(
    handle: u64,
    descriptor: OwnedFd,
    io_lock: &Mutex<()>,
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
    saved?;
    if poisoned {
        return Err(io::Error::from_raw_os_error(libc::EIO));
    }
    match close_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

impl Client {
    pub fn connect(socket: &Path, username: &str, repository_id: &str) -> io::Result<Self> {
        crate::protocol::validate_username(username)?;
        Self::connect_stream(UnixStream::connect(socket)?, username, repository_id)
    }
    fn connect_stream(stream: UnixStream, username: &str, repository_id: &str) -> io::Result<Self> {
        let client = Self(Arc::new(Mutex::new(Some(FrameStream::new(stream)?))));
        let (result, _) = client.call(Request::Hello {
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
                "SMB bridge did not negotiate authenticated single-account sessions and durable write reservations",
            ));
        }
        Ok(client)
    }
    pub fn call(&self, request: Request) -> io::Result<(Value, Option<OwnedFd>)> {
        let mut connection = self
            .0
            .lock()
            .map_err(|_| io::Error::other("RPC connection poisoned"))?;
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
                if expects_fd {
                    let Response::Ok { result } = &response else {
                        unreachable!();
                    };
                    // The remote handle already exists. If its metadata cannot
                    // be decoded, no caller can close it by identity. Terminate
                    // the session while holding the shared lock so the server
                    // releases it and no clone continues a partially adopted open.
                    json_codec::from_value::<OpenResult>(result.clone())
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                }
                Ok((response, fd))
            });
        let (response, fd) = match exchange {
            Ok(response) => response,
            Err(error) => {
                // A partial request/response or invalid frame loses message
                // alignment. Close the shared connection before unlocking so
                // every clone fails instead of consuming a stale response.
                if let Some(stream) = connection.take() {
                    let _ = stream.0.shutdown(Shutdown::Both);
                }
                return Err(match request {
                    Request::Namespace { operation_id, .. } => io::Error::new(
                        error.kind(),
                        RemoteError {
                            code: "RPC_OUTCOME_UNKNOWN".into(),
                            message: format!(
                                "{error}; query the original operation before retrying"
                            ),
                            operation_id: Some(operation_id),
                            retryable: false,
                            errno: error.raw_os_error().unwrap_or(libc::EIO),
                        },
                    ),
                    _ => error,
                });
            }
        };
        match response {
            Response::Ok { result } => Ok((result, fd)),
            Response::Error {
                code,
                message,
                operation_id,
                retryable,
            } => {
                let errno = remote_errno(&code);
                Err(io::Error::new(
                    io::Error::from_raw_os_error(errno).kind(),
                    RemoteError {
                        code,
                        message,
                        operation_id,
                        retryable,
                        errno,
                    },
                ))
            }
        }
    }
    pub fn open(&self, request: Request) -> io::Result<(OpenResult, OwnedFd)> {
        if !matches!(request, Request::Open { .. }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected open request",
            ));
        }
        let (result, fd) = self.call(request)?;
        let result = json_codec::from_value(result).map_err(io::Error::other)?;
        Ok((
            result,
            fd.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "open returned no descriptor")
            })?,
        ))
    }
}

#[cfg(test)]
#[path = "../../../test/smb/unit/client/tests.rs"]
mod tests;
