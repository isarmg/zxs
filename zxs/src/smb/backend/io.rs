//! File IO and explicit-close semantics over the original published storage
//! hold. No client-supplied ID, pathname, or backend substitute is accepted.
//! The dispatcher still has to verify the current connection/session/tree
//! and resolve both halves (or the exact previous compound OPEN) first.
use super::{Held, Object};
use std::num::NonZeroU32;
use crate::smb::wire::io_fields::{CloseMode, ReadRange, WriteData, WritePosition, file_range_end};

/// Implement on a real Core hold only. Perform this exact read and return
/// ONLY after the current grant, same held view/medium and any range locks
/// have been checked. This includes empty windows, EOF and any check after
/// staging output. A success is the exact available prefix of the held
/// logical view, not an incomplete physical read or a suppressed Core error.
/// Body must own or borrow an immutable snapshot of those same verified bytes.
/// The prepared bytes can be the actual response's data, without another
/// store or a duplicate persistent copy. No socket output occurs here.
pub(super) trait CheckedRead {
    type Body: AsRef<[u8]>;
    type Error;
    fn read_checked(&self, offset: u64, length: u32) -> Result<Self::Body, Self::Error>;
}

/// Only a successful checked READ with at least one byte produces this.
#[must_use]
pub(in crate::smb) struct ReadCompletion<B> {
    body: B,
    count: NonZeroU32,
}

impl<B: AsRef<[u8]>> ReadCompletion<B> {
    pub(in crate::smb) fn data(&self) -> &[u8] { self.body.as_ref() }
    pub(in crate::smb) fn count(&self) -> NonZeroU32 { self.count }
}

#[derive(Debug, Eq, PartialEq)]
pub(in crate::smb) enum ReadFault<E> {
    NotDataFile,
    AccessDenied,
    UnbufferedUnavailable,
    ResponseSpace,
    InconsistentLength,
    EndOfFile,
    Original(E),
}

impl<H: CheckedRead> Held<'_, H> {
    /// Only a previously bound original OPEN can invoke this. This tests
    /// its real OPEN grant, not the share-access vote (EXECUTE is not
    /// READ_DATA). The original adapter owns the second, current cut.
    /// response_room is payload room after the protocol envelope is reserved.
    pub(super) fn read(&self, range: ReadRange, response_room: usize) -> Result<ReadCompletion<H::Body>, ReadFault<H::Error>> {
        use ReadFault::*;
        if !matches!(self._slot.object, Object::File(..)) {
            return Err(NotDataFile);
        }
        if self.grant.0 & 0x0001 == 0 { // FILE_READ_DATA
            return Err(AccessDenied);
        }
        if range.unbuffered() {
            // The checked logical Core reader is buffered. No claim that a
            // verified window is direct storage IO or honors its alignment.
            return Err(UnbufferedUnavailable);
        }
        let requested = usize::try_from(range.length()).map_err(|_| ResponseSpace)?;
        if requested > response_room {
            return Err(ResponseSpace); // don't shorten a read to the local frame budget
        }
        let body = self.original.read_checked(range.offset(), range.length()).map_err(Original)?;
        let count = u32::try_from(body.as_ref().len()).map_err(|_| InconsistentLength)?;
        if count > range.length() {
            return Err(InconsistentLength);
        }
        // MS-FSA §2.1.5.3 permits the underlying zero-length read.
        // MS-SMB2 §2.2.20 nevertheless requires EOF for zero returned
        // bytes, while §3.3.5.12 requires EOF below MinimumCount.
        let count = NonZeroU32::new(count).ok_or(EndOfFile)?;
        if count.get() < range.minimum() {
            return Err(EndOfFile);
        }
        Ok(ReadCompletion { body, count })
    }
}

/// Only genuine values fetched for the original logical object; not host
/// container stat data. The adapter may return no snapshot if unavailable.
pub(in crate::smb) struct CloseAttributes {
    pub(in crate::smb) created: u64,
    pub(in crate::smb) accessed: u64,
    pub(in crate::smb) written: u64,
    pub(in crate::smb) changed: u64,
    pub(in crate::smb) allocation_size: u64,
    pub(in crate::smb) end_of_file: u64,
    pub(in crate::smb) file_attributes: u32,
}

/// Success from an explicit CLOSE after its original hold is released.
#[must_use]
pub(in crate::smb) struct CloseCompletion {
    attributes: Option<CloseAttributes>,
}

impl CloseCompletion {
    pub(in crate::smb) fn attributes(&self) -> Option<&CloseAttributes> {
        self.attributes.as_ref()
    }
}

/// Prepare an explicit CLOSE before dropping the held Core object, once
/// the OPEN is no longer routable and accepted IO has
/// been settled. A normal dirty writer close must use Core's checked
/// completion path, not silently drop its work or make a history commit.
/// None means no authorized attributes could be collected, not fake zeros
/// marked valid. This is not called by Drop, transport loss or revocation.
pub(super) trait CheckedClose {
    type Error;
    fn close_checked(&mut self, mode: CloseMode) -> Result<Option<CloseAttributes>, Self::Error>;
}

impl<H: CheckedClose> Held<'_, H> {
    #[must_use = "explicit close errors are not successful closes"]
    pub(super) fn close(mut self, mode: CloseMode) -> Result<CloseCompletion, H::Error> {
        let finished = self.original.close_checked(mode);
        drop(self); // even on error, the real original closes before its share vote falls
        finished.map(|attrs| CloseCompletion {
            attributes: if mode == CloseMode::PostQueryAttributes { attrs } else { None },
        })
    }
}

/// Same actual single-writer hold as CREATE. The current EOF is the private
/// working view of that writer, checked under the current authority, not a
/// confirmed reader's length. Calls run in order under one mutable OPEN.
/// The adapter still checks current tree/session/medium, preserves the exact
/// write range against any byte locks, and keeps a pending batch on failure.
pub(super) trait CheckedWrite {
    type Error;
    fn writer_eof_checked(&self) -> Result<u64, Self::Error>;
    fn write_open_mode(&self) -> WriteOpenMode; // actual CREATE mode
    /// All exact bytes accepted to this writer, or error. Zero bytes still
    /// check current authority but do not write, resize or invent a revision.
    fn write_checked(&mut self, offset: u64, input: &[u8]) -> Result<u32, Self::Error>;
    /// On success the original working revision is clean or confirmed by
    /// Core. This is neither a named history node nor an operation retry.
    fn flush_checked(&mut self) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WriteOpenMode {
    Buffered,
    BufferedWriteThrough, // FILE_WRITE_THROUGH on the original open
    Unbuffered, // FILE_NO_INTERMEDIATE_BUFFERING; no such IO implemented here
}

/// Acknowledgement of an exact original WRITE; not a history commit.
#[must_use]
#[derive(Debug, Eq, PartialEq)]
pub(in crate::smb) struct WriteCompletion {
    count: u32,
    confirmed: bool,
}

impl WriteCompletion {
    pub(in crate::smb) fn count(&self) -> u32 { self.count }
}

/// Explicit original FLUSH completed, either already clean or confirmed.
#[must_use]
pub(in crate::smb) struct FlushCompletion { _checked: () }

#[derive(Debug, Eq, PartialEq)]
pub(in crate::smb) enum WriteFault<E> {
    AccessDenied,
    NotDataFile,
    DirectoryFlushUnavailable,
    InvalidWriteCombination,
    UnbufferedUnavailable,
    InvalidRange,
    InconsistentLength,
    Original(E),
    Confirmation(E),
}

impl<H: CheckedWrite> Held<'_, H> {
    pub(super) fn write(&mut self, request: WriteData<'_>) -> Result<WriteCompletion, WriteFault<H::Error>> {
        use WriteFault::*;
        const WRITE_DATA: u32 = 0x0002;
        const APPEND_DATA: u32 = 0x0004;
        if !matches!(self._slot.object, Object::File(..)) {
            return Err(NotDataFile);
        }
        let can_write = self.grant.0 & WRITE_DATA != 0;
        let can_append = self.grant.0 & APPEND_DATA != 0;
        if !can_write && !can_append {
            return Err(AccessDenied);
        }
        let mode = self.original.write_open_mode();
        let unbuffered_open = mode == WriteOpenMode::Unbuffered;
        // 3.1.1 per-request WRITE_THROUGH is not the open's FILE_WRITE_THROUGH
        // option. Check the combination in MS-SMB2 §3.3.5.13 before IO.
        if request.write_through() && !request.unbuffered() && !unbuffered_open {
            return Err(InvalidWriteCombination);
        }
        if request.unbuffered() || unbuffered_open {
            return Err(UnbufferedUnavailable);
        }

        let eof = self.original.writer_eof_checked().map_err(Original)?;
        let offset = match request.position() {
            WritePosition::At(at) => at,
            WritePosition::EndOfFile => eof,
        };
        let end = file_range_end(offset, request.length()).ok_or(InvalidRange)?;
        // Append-only can act at the original writer's current EOF, never
        // overwrite or silently replace a different explicit offset. A real
        // extension also requires APPEND_DATA in this profile (§3.3.5.13).
        if (!can_write && offset != eof)
            || (!can_append && request.length() != 0 && end > eof)
        {
            return Err(AccessDenied);
        }
        let confirm = mode == WriteOpenMode::BufferedWriteThrough;
        let count = self.original.write_checked(offset, request.payload()).map_err(Original)?;
        if count != request.length() {
            // This Core contract admits the exact range or fails. Do not
            // invent a full count, assume rollback, or resend a partial write.
            return Err(InconsistentLength);
        }
        if confirm {
            self.original.flush_checked().map_err(Confirmation)?;
        }
        Ok(WriteCompletion { count, confirmed: confirm })
    }

    pub(super) fn flush(&mut self) -> Result<FlushCompletion, WriteFault<H::Error>> {
        use WriteFault::*;
        // For a FILE these are WRITE/APPEND_DATA; for a directory these are
        // ADD_FILE/ADD_SUBDIRECTORY. Never use directory rights to flush FILE
        // data or invent a host-volume sync for a logical directory.
        if self.grant.0 & 0x0006 == 0 {
            return Err(AccessDenied);
        }
        if !matches!(self._slot.object, Object::File(..)) {
            return Err(DirectoryFlushUnavailable);
        }
        self.original.flush_checked().map_err(Confirmation)?;
        Ok(FlushCompletion { _checked: () })
    }
}

#[cfg(test)]
#[path = "../../test/smb/backend_io.rs"]
mod tests;
