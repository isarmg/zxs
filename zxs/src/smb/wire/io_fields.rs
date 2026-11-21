//! READ, WRITE, FLUSH and CLOSE request views from a single SMB2 member.
//! An OpenReference describes where to look, never what is authorized.
use super::{bytes, message::{Message, RELATED}, u16le, u32le, u64le};

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
    ExceedsMaxWrite,
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

/// Symbolic EOF is resolved under the original writer, never against a
/// directory's cached length or an unrelated read handle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum WritePosition {
    At(u64),
    EndOfFile,
}

// No Debug: the slice is actual file data once wired.
#[derive(Clone, Copy)]
pub(in crate::smb) struct WriteData<'a> {
    position: WritePosition,
    payload: &'a [u8],
    length: u32,
    write_through: bool,
    unbuffered: bool,
}

impl<'a> WriteData<'a> {
    pub(in crate::smb) fn position(self) -> WritePosition { self.position }
    pub(in crate::smb) fn payload(self) -> &'a [u8] { self.payload }
    /// Charge the requested bytes, not the count that ultimately succeeds.
    pub(in crate::smb) fn length(self) -> u32 { self.length }
    pub(in crate::smb) fn write_through(self) -> bool { self.write_through }
    pub(in crate::smb) fn unbuffered(self) -> bool { self.unbuffered }
}

#[derive(Clone, Copy)]
pub(in crate::smb) struct WriteRequest<'a> {
    pub(in crate::smb) open: OpenReference,
    pub(in crate::smb) data: WriteData<'a>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) struct FlushRequest {
    pub(in crate::smb) open: OpenReference,
}

/// Check just the signed wire file domain; neither a grant nor a logical EOF.
pub(in crate::smb) fn file_range_end(offset: u64, length: u32) -> Option<u64> {
    offset.checked_add(u64::from(length)).filter(|end| *end <= i64::MAX as u64)
}

const WRITE_FIXED: usize = 112;

impl<'a> Message<'a> {
    pub(super) fn io_open_reference(&self, offset: usize) -> Result<OpenReference, IoFieldError> {
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
        self.read_fixed()?;
        let p = self.raw;
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
        if file_range_end(offset, length).is_none() {
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

    /// This is only the bounded raw routing coordinate; the already-held
    /// object later checks the exact paid READ with its real session limits.
    pub(in crate::smb) fn read_target(&self)->Result<OpenReference,IoFieldError>{
        self.read_fixed()?;
        self.io_open_reference(80)
    }
    fn read_fixed(&self)->Result<(),IoFieldError>{
        use IoFieldError::*;
        if self.header.command!=0x0008{return Err(WrongCommand);}
        if self.raw.len()<112||u16le(self.raw,64).map_err(|_|Malformed)?!=49{return Err(Malformed);}
        Ok(())
    }

    pub(in crate::smb) fn close_request(&self) -> Result<CloseRequest, IoFieldError> {
        use IoFieldError::*;
        self.close_fixed()?;
        let p = self.raw;
        let mode = match u16le(p, 66).map_err(|_| Malformed)? {
            0 => CloseMode::Plain,
            1 => CloseMode::PostQueryAttributes,
            _ => return Err(Malformed),
        };
        // The reserved word at 68 is ignored, not another namespace or mode.
        Ok(CloseRequest { open: self.io_open_reference(72)?, mode })
    }

    /// Bound the target before resolving the original CLOSE. Its flags are
    /// processed by the actual object after Session.OpenTable and Tree lookup.
    pub(in crate::smb) fn close_target(&self)->Result<OpenReference,IoFieldError>{
        self.close_fixed()?;
        self.io_open_reference(72)
    }
    fn close_fixed(&self)->Result<(),IoFieldError>{
        use IoFieldError::*;
        if self.header.command!=0x0006{return Err(WrongCommand);}
        if self.raw.len()<88||u16le(self.raw,64).map_err(|_|Malformed)?!=24{return Err(Malformed);}
        Ok(())
    }

    /// MS-SMB2 §§2.2.21 and 3.3.5.13. Borrow exactly Length TCP bytes of this
    /// message, never a descriptor, an adjacent command, or compound padding.
    pub(in crate::smb) fn write_request(&self, max_write: u32) -> Result<WriteRequest<'a>, IoFieldError> {
        use IoFieldError::*;
        self.write_fixed()?;
        let p = self.raw;
        if u32le(p, 96).map_err(|_| Malformed)? != 0 {
            return Err(Malformed); // TCP only, including when Length is zero
        }
        let length = u32le(p, 68).map_err(|_| Malformed)?;
        if length > max_write {
            return Err(ExceedsMaxWrite);
        }
        let at = usize::from(u16le(p, 66).map_err(|_| Malformed)?);
        let payload = if length == 0 && at == 0 {
            &p[0..0]
        } else {
            if !(WRITE_FIXED..=0x100).contains(&at) {
                return Err(Malformed);
            }
            bytes(p, at, usize::try_from(length).map_err(|_| Malformed)?).map_err(|_| Malformed)?
        };
        let raw_offset = u64le(p, 72).map_err(|_| Malformed)?;
        let position = if raw_offset == u64::MAX {
            WritePosition::EndOfFile // FILE_WRITE_TO_END_OF_FILE
        } else if file_range_end(raw_offset, length).is_some() {
            WritePosition::At(raw_offset)
        } else {
            return Err(InvalidRange); // no implicit current-file-position mode
        };
        let flags = u32le(p, 108).map_err(|_| Malformed)?;
        // §3.3.5.13 says to ignore undefined flag bits; CHANNEL_NONE also
        // requires ignoring RemainingBytes and both channel-data fields.
        Ok(WriteRequest {
            open: self.io_open_reference(80)?,
            data: WriteData {
                position, payload, length, write_through: flags & 1 != 0, unbuffered: flags & 2 != 0,
            },
        })
    }

    /// The fixed routing coordinate only; the original session/tree/OPEN
    /// lookup precedes length, channel and buffer validation (§3.3.5.13).
    /// A successful lookup here never makes malformed WRITE data executable.
    pub(in crate::smb) fn write_target(&self) -> Result<OpenReference, IoFieldError> {
        self.write_fixed()?;
        self.io_open_reference(80)
    }
    fn write_fixed(&self) -> Result<(), IoFieldError> {
        use IoFieldError::*;
        if self.header.command != 0x0009 { return Err(WrongCommand); }
        if self.raw.len() < WRITE_FIXED || u16le(self.raw, 64).map_err(|_| Malformed)? != 49 {
            return Err(Malformed);
        }
        Ok(())
    }

    /// MS-SMB2 §2.2.17. Neither reserved field is a flush mode.
    pub(in crate::smb) fn flush_request(&self) -> Result<FlushRequest, IoFieldError> {
        use IoFieldError::*;
        if self.header.command != 0x0007 {
            return Err(WrongCommand);
        }
        if self.raw.len() < 88 || u16le(self.raw, 64).map_err(|_| Malformed)? != 24 {
            return Err(Malformed);
        }
        Ok(FlushRequest { open: self.io_open_reference(72)? })
    }
}
