//! Read and explicit-close semantics over the original published storage
//! hold. No client-supplied ID, pathname, or backend substitute is accepted.
//! The dispatcher still has to verify the current connection/session/tree
//! and resolve both halves (or the exact previous compound OPEN) first.
use super::{Held, Object};
use crate::smb::wire::io_fields::{CloseMode, ReadRange};

/// Implement on a real Core hold only. Perform this exact read and return
/// ONLY after the current grant, same held view/medium and any range locks
/// have been checked. This includes empty windows, EOF and any check after
/// staging output. A success is the exact available prefix of the held
/// logical view, not an incomplete physical read or a suppressed Core error.
/// The prepared bytes can be the actual response's data, without another
/// store or a duplicate persistent copy. No socket output occurs here.
pub(super) trait CheckedRead {
    type Body: AsRef<[u8]>;
    type Error;
    fn read_checked(&self, offset: u64, length: u32) -> Result<Self::Body, Self::Error>;
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum ReadFault<E> {
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
    pub(super) fn read(&self, range: ReadRange, response_room: usize) -> Result<H::Body, ReadFault<H::Error>> {
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
        // MS-FSA §2.1.5.3 permits zero-length reads, but a positive
        // request at verified EOF isn't a successful empty data response.
        // MS-SMB2 §3.3.5.12 also distinguishes a short successful read
        // from a result shorter than the original MinimumCount.
        if count < range.minimum() || (count == 0 && range.length() != 0) {
            return Err(EndOfFile);
        }
        Ok(body)
    }
}

/// Only genuine values fetched for the original logical object; not host
/// container stat data. The adapter may return no snapshot if unavailable.
pub(super) struct CloseAttributes {
    pub(super) created: u64,
    pub(super) accessed: u64,
    pub(super) written: u64,
    pub(super) changed: u64,
    pub(super) allocation_size: u64,
    pub(super) end_of_file: u64,
    pub(super) file_attributes: u32,
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
    pub(super) fn close(mut self, mode: CloseMode) -> Result<Option<CloseAttributes>, H::Error> {
        let finished = self.original.close_checked(mode);
        drop(self); // even on error, the real original closes before its share vote falls
        finished.map(|attrs| if mode == CloseMode::PostQueryAttributes { attrs } else { None })
    }
}

#[cfg(test)]
#[path = "../../test/smb/backend_io.rs"]
mod tests;
