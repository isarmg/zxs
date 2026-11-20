//! READ and CLOSE request views (MS-SMB2 §§2.2.19 and 2.2.15).
//! An OpenReference describes where to look, never what is authorized.
use super::{message::{Message, RELATED}, u16le, u32le, u64le};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::smb) struct WireOpenId {
    pub(in crate::smb) persistent: u64,
    pub(in crate::smb) volatile: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum OpenReference {
    /// Look up both halves within the authenticated connection/session/tree.
    Explicit(WireOpenId),
    /// Only the immediately preceding operation's proved IDs in this same
    /// related chain count. Never fall back to an explicit or "last" OPEN.
    PreviousInCompound,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum IoFieldError {
    WrongCommand,
    Malformed,
    NoExplicitOpen,
    ExceedsMaxRead,
    InvalidRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) struct ReadRange {
    offset: u64,
    length: u32,
    minimum: u32,
    unbuffered: bool,
}

impl ReadRange {
    pub(in crate::smb) fn offset(self) -> u64 { self.offset }
    /// Also the complete expected response size for credit checking. Never
    /// substitute a page/EOF-shortened response for this request length.
    pub(in crate::smb) fn length(self) -> u32 { self.length }
    pub(in crate::smb) fn minimum(self) -> u32 { self.minimum }
    pub(in crate::smb) fn unbuffered(self) -> bool { self.unbuffered }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) struct ReadRequest {
    pub(in crate::smb) open: OpenReference,
    pub(in crate::smb) range: ReadRange,
    /// An eligible response is compressible only on a connection that
    /// actually negotiated and implements it, independent of this hint.
    pub(in crate::smb) compression_requested: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum CloseMode {
    Plain,
    PostQueryAttributes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) struct CloseRequest {
    pub(in crate::smb) open: OpenReference,
    pub(in crate::smb) mode: CloseMode,
}

impl Message<'_> {
    fn io_open_reference(&self, offset: usize) -> Result<OpenReference, IoFieldError> {
        // §3.3.5.2.7.2 says to use the prior operation's IDs in all
        // related members, not only if the client sent a sentinel.
        if self.header.flags & RELATED != 0 {
            return Ok(OpenReference::PreviousInCompound);
        }
        let persistent = u64le(self.raw, offset).map_err(|_| IoFieldError::Malformed)?;
        let volatile = u64le(self.raw, offset + 8).map_err(|_| IoFieldError::Malformed)?;
        if persistent == u64::MAX && volatile == u64::MAX {
            return Err(IoFieldError::NoExplicitOpen);
        }
        Ok(OpenReference::Explicit(WireOpenId { persistent, volatile }))
    }

    pub(in crate::smb) fn read_request(&self, max_read: u32) -> Result<ReadRequest, IoFieldError> {
        use IoFieldError::*;
        if self.header.command != 0x0008 {
            return Err(WrongCommand);
        }
        let p = self.raw;
        if p.len() < 112 || u16le(p, 64).map_err(|_| Malformed)? != 49 {
            return Err(Malformed);
        }
        let flags = p[67];
        if flags & !0x03 != 0 || u32le(p, 100).map_err(|_| Malformed)? != 0 {
            // This profile is TCP. An RDMA channel descriptor on TCP is an
            // invalid request (§3.3.5.12), not an alternate input buffer.
            return Err(Malformed);
        }
        // Ignore padding and all three channel-info fields for CHANNEL_NONE.
        let length = u32le(p, 68).map_err(|_| Malformed)?;
        if length > max_read {
            return Err(ExceedsMaxRead);
        }
        let offset = u64le(p, 72).map_err(|_| Malformed)?;
        if offset.checked_add(u64::from(length)).is_none_or(|end| end > i64::MAX as u64) {
            return Err(InvalidRange);
        }
        let minimum = u32le(p, 96).map_err(|_| Malformed)?;
        // A minimum larger than what the read yields is EOF after checked
        // IO, not an invented range or a reason to skip current permission.
        Ok(ReadRequest {
            open: self.io_open_reference(80)?,
            range: ReadRange { offset, length, minimum, unbuffered: flags & 1 != 0 },
            compression_requested: flags & 2 != 0,
        })
    }

    pub(in crate::smb) fn close_request(&self) -> Result<CloseRequest, IoFieldError> {
        use IoFieldError::*;
        if self.header.command != 0x0006 {
            return Err(WrongCommand);
        }
        let p = self.raw;
        if p.len() < 88 || u16le(p, 64).map_err(|_| Malformed)? != 24 {
            return Err(Malformed);
        }
        let mode = match u16le(p, 66).map_err(|_| Malformed)? {
            0 => CloseMode::Plain,
            1 => CloseMode::PostQueryAttributes,
            _ => return Err(Malformed),
        };
        // The reserved word at 68 is ignored, not another namespace or mode.
        Ok(CloseRequest { open: self.io_open_reference(72)?, mode })
    }
}
