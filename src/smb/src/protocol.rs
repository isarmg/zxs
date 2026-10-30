use data_serialization::{Deserialize, Serialize, de::DeserializeOwned};
use json_codec::Value;
use std::{
    io, mem,
    os::{
        fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    time::{Duration, Instant},
};

pub const PROTOCOL: &str = "smb-protocol-rpc";
/// Writable descriptors are safe to admit only when both peers require a
/// durable PrepareWrite reservation before every descriptor mutation.
pub const WRITE_RESERVATION_CAPABILITY: &str = "durable-write-reservations";
/// Upgrade gate, not an authentication proof. Only a trusted local Samba peer
/// may attest this behavior after checking its actual authenticated session.
pub const AUTHENTICATED_ACCOUNT_CAPABILITY: &str = "authenticated-single-account";
/// `PrepareWrite` authorizes every descriptor mutation; `Dirty` is sent only
/// when a mutation opens a save generation (first change after open or Flush)
/// and for every write-through change. See `client::WriteSequence`.
pub const GENERATION_DIRTY_CAPABILITY: &str = "generation-dirty-writes";
/// Immutable reads use range RPCs; Open hands out a non-content placeholder FD.
pub const VERIFIED_READ_CAPABILITY: &str = "verified-range-reads";
pub const REQUIRED_CAPABILITIES: [&str; 4] = [
    WRITE_RESERVATION_CAPABILITY,
    AUTHENTICATED_ACCOUNT_CAPABILITY,
    GENERATION_DIRTY_CAPABILITY,
    VERIFIED_READ_CAPABILITY,
];

/// A single exact Samba account name, never an alias-to-principal mapping.
pub fn validate_username(username: &str) -> io::Result<()> {
    if username.is_empty()
        || username.len() > 256
        || username.trim() != username
        || username.chars().any(char::is_control)
        || username.contains('=')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SMB requires one exact authenticated Samba account name, not a mapping",
        ));
    }
    Ok(())
}

pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
/// JSON byte arrays use at most four encoded bytes per byte plus small metadata.
pub const MAX_READ_BYTES: usize = 64 * 1024;
/// Directory replies stay well below the frame ceiling, even with long names.
pub const MAX_DIRECTORY_PAGE_BYTES: usize = 256 * 1024;
pub const MAX_DIRECTORY_PAGE_ENTRIES: usize = 256;

pub fn effective_uid() -> u32 {
    // SAFETY: geteuid has no arguments or side effects and cannot fail.
    unsafe { libc::geteuid() }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request<O = String> {
    Hello {
        protocol: String,
        capabilities: Vec<String>,
        username: String,
        repository_id: String,
    },
    Open {
        path: String,
        writable: bool,
        create: bool,
        truncate: bool,
        exclusive: bool,
        write_through: bool,
    },
    /// Admit one descriptor mutation before FD I/O: current authorization,
    /// the durable write ceiling and recovery headroom for these bytes.
    PrepareWrite {
        #[serialization(with = "storage_contract::u64_string")]
        handle: u64,
        #[serialization(with = "storage_contract::u64_string")]
        end: u64,
        #[serialization(with = "storage_contract::u64_string")]
        bytes: u64,
    },
    Dirty {
        #[serialization(with = "storage_contract::u64_string")]
        handle: u64,
    },
    AuthorizeRead {
        #[serialization(with = "storage_contract::u64_string")]
        handle: u64,
    },
    /// Return only verified immutable bytes; admit current identity and binding
    /// at actual execution, including cache hits and empty/EOF reads.
    Read {
        #[serialization(with = "storage_contract::u64_string")]
        handle: u64,
        #[serialization(with = "storage_contract::u64_string")]
        offset: u64,
        length: u32,
    },
    Flush {
        #[serialization(with = "storage_contract::u64_string")]
        handle: u64,
    },
    Close {
        #[serialization(with = "storage_contract::u64_string")]
        handle: u64,
    },
    /// Release a handle without publishing or discarding its retained work.
    Detach {
        #[serialization(with = "storage_contract::u64_string")]
        handle: u64,
    },
    Abort {
        #[serialization(with = "storage_contract::u64_string")]
        handle: u64,
    },
    Stat {
        path: String,
    },
    ReadDirectory {
        #[serialization(with = "storage_contract::u64_string")]
        handle: u64,
        restart: bool,
    },
    Namespace {
        operation_id: O,
        kind: NamespaceOperation,
        path: String,
        destination: Option<String>,
        replace: bool,
    },
}

impl Request<String> {
    pub fn parse_operation_id(
        self,
    ) -> storage_contract::Result<Request<storage_contract::OperationId>> {
        Ok(match self {
            Self::Hello {
                protocol,
                capabilities,
                username,
                repository_id,
            } => Request::Hello {
                protocol,
                capabilities,
                username,
                repository_id,
            },
            Self::Open {
                path,
                writable,
                create,
                truncate,
                exclusive,
                write_through,
            } => Request::Open {
                path,
                writable,
                create,
                truncate,
                exclusive,
                write_through,
            },
            Self::PrepareWrite { handle, end, bytes } => {
                Request::PrepareWrite { handle, end, bytes }
            }
            Self::Dirty { handle } => Request::Dirty { handle },
            Self::AuthorizeRead { handle } => Request::AuthorizeRead { handle },
            Self::Read {
                handle,
                offset,
                length,
            } => Request::Read {
                handle,
                offset,
                length,
            },
            Self::Flush { handle } => Request::Flush { handle },
            Self::Close { handle } => Request::Close { handle },
            Self::Detach { handle } => Request::Detach { handle },
            Self::Abort { handle } => Request::Abort { handle },
            Self::Stat { path } => Request::Stat { path },
            Self::ReadDirectory { handle, restart } => Request::ReadDirectory { handle, restart },
            Self::Namespace {
                operation_id,
                kind,
                path,
                destination,
                replace,
            } => Request::Namespace {
                operation_id: operation_id.parse()?,
                kind,
                path,
                destination,
                replace,
            },
        })
    }
}

/// Namespace operations admitted by the bridge protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum NamespaceOperation {
    Mkdir,
    Rename,
    Unlink,
    Rmdir,
}

impl<'de> Deserialize<'de> for NamespaceOperation {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "mkdir" => Ok(Self::Mkdir),
            "rename" => Ok(Self::Rename),
            "unlink" => Ok(Self::Unlink),
            "rmdir" => Ok(Self::Rmdir),
            _ => Err(data_serialization::de::Error::unknown_variant(
                &value,
                &["mkdir", "rename", "unlink", "rmdir"],
            )),
        }
    }
}

/// Allocate the identity before any I/O so a lost reply remains queryable.
/// This is a new operation, not an automatic retry of an unknown outcome.
pub fn namespace_request(
    kind: NamespaceOperation,
    path: String,
    destination: Option<String>,
    replace: bool,
) -> Request<storage_contract::OperationId> {
    Request::Namespace {
        operation_id: storage_contract::OperationId::generate(),
        kind,
        path,
        destination,
        replace,
    }
}

/// Preserve unlinkat's target-kind contract across the RPC boundary.
/// Unknown flags are rejected before a request can mutate the namespace.
pub fn remove_request(
    path: String,
    flags: i32,
) -> io::Result<Request<storage_contract::OperationId>> {
    if flags & !libc::AT_REMOVEDIR != 0 {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    Ok(namespace_request(
        if flags & libc::AT_REMOVEDIR != 0 {
            NamespaceOperation::Rmdir
        } else {
            NamespaceOperation::Unlink
        },
        path,
        None,
        false,
    ))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Ok {
        result: Value,
    },
    Error {
        code: storage_contract::ErrorCode,
        message: String,
        retryable: bool,
        operation_id: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct OpenResult {
    pub entry_id: String,
    pub kind: storage_contract::NamespaceEntryKind,
    pub path: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub handle: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub size: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub max_file_bytes: u64,
    pub version_id: Option<String>,
    pub workspace_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ReadResult {
    pub bytes: Vec<u8>,
}

impl OpenResult {
    /// Immutable handles expose only the verified logical extent, even if the
    /// underlying inode is later extended. Writable workspace descriptors must
    /// instead observe their current length, not the size captured at open.
    pub fn read_count(&self, writable: bool, offset: u64, requested: usize) -> usize {
        if writable {
            requested
        } else {
            requested.min(self.size.saturating_sub(offset).min(usize::MAX as u64) as usize)
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryPage {
    pub entries: Vec<storage_contract::Entry>,
    pub more: bool,
}

/// One serialized request/response stream. The descriptor, when present, is
/// attached to the four-byte frame header using SCM_RIGHTS, never a JSON path.
pub struct FrameStream(pub UnixStream, Duration);

/// Only an authenticated server session may wait indefinitely for a new frame.
/// Handshakes and replies must arrive within the configured frame deadline.
#[derive(Clone, Copy)]
pub enum ReadMode {
    Deadline,
    EstablishedSession,
}

impl FrameStream {
    pub fn new(stream: UnixStream) -> io::Result<Self> {
        Self::with_timeout(stream, Duration::from_secs(120))
    }

    pub fn with_timeout(stream: UnixStream, timeout: Duration) -> io::Result<Self> {
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        Ok(Self(stream, timeout))
    }

    pub fn send<T: Serialize>(&mut self, value: &T, fd: Option<BorrowedFd<'_>>) -> io::Result<()> {
        let payload = json_codec::to_vec(value).map_err(io::Error::other)?;
        self.send_serialized(&payload, fd)
    }

    /// Send an already serialized JSON payload, rejecting invalid frame sizes
    /// before writing any bytes or transferring the borrowed descriptor.
    pub fn send_serialized(
        &mut self,
        payload: &[u8],
        fd: Option<BorrowedFd<'_>>,
    ) -> io::Result<()> {
        if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "RPC frame size exceeded",
            ));
        }
        let deadline = self.deadline()?;
        let header = (payload.len() as u32).to_be_bytes();
        let mut vector = libc::iovec {
            iov_base: header.as_ptr().cast_mut().cast(),
            iov_len: header.len(),
        };
        // usize provides the alignment required by cmsghdr. Space for one int
        // plus padding fits this fixed, initialized 64-byte control buffer.
        let mut control = [0usize; 8];
        // SAFETY: zero is the initialized empty msghdr representation; all used
        // pointers below refer to live header/control storage until sendmsg returns.
        let mut message: libc::msghdr = unsafe { mem::zeroed() };
        message.msg_iov = &mut vector;
        message.msg_iovlen = 1;
        if let Some(fd) = fd {
            message.msg_control = control.as_mut_ptr().cast();
            // SAFETY: libc computes the native ancillary buffer layout for one fd.
            message.msg_controllen =
                unsafe { libc::CMSG_SPACE(mem::size_of::<i32>() as u32) as usize };
            // SAFETY: control is aligned, zeroed, and large enough for this header
            // and one descriptor; the ancillary pointers do not escape this call.
            unsafe {
                let cmsg = libc::CMSG_FIRSTHDR(&message);
                (*cmsg).cmsg_level = libc::SOL_SOCKET;
                (*cmsg).cmsg_type = libc::SCM_RIGHTS;
                (*cmsg).cmsg_len = libc::CMSG_LEN(mem::size_of::<i32>() as u32) as usize;
                std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<i32>(), fd.as_raw_fd());
            }
        }
        let sent = loop {
            remaining_timeout(deadline)?;
            // SAFETY: all message pointers and lengths were initialized above;
            // the socket is a live owned Unix stream. MSG_NOSIGNAL avoids SIGPIPE.
            let result = unsafe {
                libc::sendmsg(
                    self.0.as_raw_fd(),
                    &message,
                    libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT,
                )
            };
            if result >= 0 {
                break result as usize;
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => self.wait_writable(deadline)?,
                _ => return Err(error),
            }
        };
        if sent == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        self.write_before_deadline(&header[sent..], deadline)?;
        self.write_before_deadline(payload, deadline)
    }

    // A blocking send can keep making progress beyond SO_SNDTIMEO. Use
    // per-call nonblocking writes (without changing shared descriptor flags)
    // and one poll deadline for the complete header and payload.
    fn write_before_deadline(&mut self, mut bytes: &[u8], deadline: Instant) -> io::Result<()> {
        while !bytes.is_empty() {
            remaining_timeout(deadline)?;
            // SAFETY: bytes is live readable storage and the socket is owned.
            let count = unsafe {
                libc::send(
                    self.0.as_raw_fd(),
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT,
                )
            };
            if count == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            if count > 0 {
                bytes = &bytes[count as usize..];
                continue;
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => self.wait_writable(deadline)?,
                _ => return Err(error),
            }
        }
        Ok(())
    }

    fn wait_writable(&self, deadline: Instant) -> io::Result<()> {
        loop {
            let remaining = remaining_timeout(deadline)?;
            // Round up so sub-millisecond waits do not spin; the caller checks
            // the absolute deadline again before attempting another write.
            let millis = remaining
                .as_millis()
                .saturating_add(1)
                .min(i32::MAX as u128) as i32;
            let mut event = libc::pollfd {
                fd: self.0.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            // SAFETY: poll receives one live writable pollfd, not an array.
            let ready = unsafe { libc::poll(&mut event, 1, millis) };
            if ready > 0 {
                // HUP/ERR also wake poll. The next send reports the OS error.
                return Ok(());
            }
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }

    pub fn receive<T: DeserializeOwned>(&mut self) -> io::Result<(T, Option<OwnedFd>)> {
        self.receive_with_mode(ReadMode::Deadline)
    }

    pub fn receive_with_mode<T: DeserializeOwned>(
        &mut self,
        mode: ReadMode,
    ) -> io::Result<(T, Option<OwnedFd>)> {
        let mut deadline = match mode {
            ReadMode::Deadline => Some(self.deadline()?),
            ReadMode::EstablishedSession => None,
        };
        let mut header = [0u8; 4];
        let mut received = 0;
        let mut descriptor = None;
        while received < header.len() {
            self.set_remaining_timeout(deadline)?;
            let mut vector = libc::iovec {
                iov_base: header[received..].as_mut_ptr().cast(),
                iov_len: header.len() - received,
            };
            let mut control = [0usize; 32];
            // SAFETY: a zeroed msghdr has no dangling allocations; its buffers are
            // fully initialized below and remain live through recvmsg.
            let mut message: libc::msghdr = unsafe { mem::zeroed() };
            message.msg_iov = &mut vector;
            message.msg_iovlen = 1;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = mem::size_of_val(&control);
            // SAFETY: all buffers are writable, bounded, and aligned. Received
            // descriptors are CLOEXEC immediately, before any other thread can fork.
            let count =
                unsafe { libc::recvmsg(self.0.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if count == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            let mut extra_descriptors = false;
            // SAFETY: libc created this ancillary sequence within the supplied
            // aligned buffer. Every SCM_RIGHTS descriptor is immediately owned,
            // including unwanted extras, so errors cannot leak received fds.
            unsafe {
                let mut cmsg = libc::CMSG_FIRSTHDR(&message);
                while !cmsg.is_null() {
                    if (*cmsg).cmsg_level == libc::SOL_SOCKET
                        && (*cmsg).cmsg_type == libc::SCM_RIGHTS
                    {
                        let base = libc::CMSG_LEN(0) as usize;
                        if (*cmsg).cmsg_len < base {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "invalid ancillary header",
                            ));
                        }
                        let bytes = (*cmsg).cmsg_len - base;
                        for offset in (0..bytes).step_by(mem::size_of::<i32>()) {
                            if offset + mem::size_of::<i32>() > bytes {
                                break;
                            }
                            let raw = std::ptr::read_unaligned(
                                libc::CMSG_DATA(cmsg).add(offset).cast::<i32>(),
                            );
                            let fd = OwnedFd::from_raw_fd(raw);
                            if descriptor.is_some() {
                                extra_descriptors = true;
                            } else {
                                descriptor = Some(fd);
                            }
                        }
                    }
                    cmsg = libc::CMSG_NXTHDR(&message, cmsg);
                }
            }
            if extra_descriptors || message.msg_flags & libc::MSG_CTRUNC != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "too many RPC descriptors",
                ));
            }
            if deadline.is_none() {
                deadline = Some(self.deadline()?);
            }
            received += count as usize;
        }
        let length = u32::from_be_bytes(header) as usize;
        if length == 0 || length > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "RPC frame size exceeded",
            ));
        }
        let mut payload = vec![0u8; length];
        let mut received = 0;
        while received < payload.len() {
            self.set_remaining_timeout(deadline)?;
            let mut vector = libc::iovec {
                iov_base: payload[received..].as_mut_ptr().cast(),
                iov_len: payload.len() - received,
            };
            // SAFETY: zero initializes an empty message. The payload slice is
            // live writable storage until recvmsg returns. No control buffer is
            // supplied: Linux closes any received rights that do not fit and
            // reports MSG_CTRUNC, allowing us to reject misplaced descriptors
            // rather than silently accepting them through read().
            let mut message: libc::msghdr = unsafe { mem::zeroed() };
            message.msg_iov = &mut vector;
            message.msg_iovlen = 1;
            // SAFETY: message points to the initialized bounded payload buffer;
            // the socket is live and ancillary data cannot escape into raw FDs.
            let count =
                unsafe { libc::recvmsg(self.0.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if count == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            if message.msg_flags & libc::MSG_CTRUNC != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "RPC descriptors must accompany the frame header",
                ));
            }
            received += count as usize;
        }
        let value = storage_contract::decode_strict(&payload).map_err(io::Error::other)?;
        Ok((value, descriptor))
    }

    fn deadline(&self) -> io::Result<Instant> {
        Instant::now().checked_add(self.1).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "RPC timeout exceeds the monotonic clock range",
            )
        })
    }

    fn set_remaining_timeout(&self, deadline: Option<Instant>) -> io::Result<()> {
        let remaining = deadline.map(remaining_timeout).transpose()?;
        self.0.set_read_timeout(remaining)
    }
}

fn remaining_timeout(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))
}

#[cfg(test)]
#[path = "../../../test/smb/unit/protocol/tests.rs"]
mod tests;
