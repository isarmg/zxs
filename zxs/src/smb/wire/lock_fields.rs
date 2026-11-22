//! One standalone nonblocking LOCK element. Decoding grants no access.
use super::{
    io_fields::WireOpenId,
    message::{Message, RELATED, Routing},
    u16le, u32le, u64le,
};
use crate::smb::connection::sessions::ordinary::VerifiedMember;

pub(in crate::smb) const LOCK: u16 = 10;
const REPLAY: u32 = 0x2000_0000;
const FIXED: usize = 24;
const ELEMENT: usize = 24;

/// A nonempty unsigned byte range, including a possible byte at `u64::MAX`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) struct Span {
    start: u64,
    last: u64,
}

impl Span {
    pub(in crate::smb) fn new(start: u64, length: u64) -> Option<Self> {
        let last = start.checked_add(length.checked_sub(1)?)?;
        Some(Self { start, last })
    }

    pub(in crate::smb) fn overlaps(self, other: Self) -> bool {
        self.start <= other.last && other.start <= self.last
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum LockChange {
    Shared,
    Exclusive,
    Unlock,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) struct LockFields {
    open: WireOpenId,
    span: Span,
    change: LockChange,
}

impl LockFields {
    pub(in crate::smb) fn open(self) -> WireOpenId {
        self.open
    }
    pub(in crate::smb) fn span(self) -> Span {
        self.span
    }
    pub(in crate::smb) fn change(self) -> LockChange {
        self.change
    }
}

#[must_use]
pub(in crate::smb) struct LockRequest<'step, 'raw> {
    member: VerifiedMember<'step, 'raw>,
    fields: LockFields,
}

impl<'step, 'raw> LockRequest<'step, 'raw> {
    pub(in crate::smb) fn parse(member: VerifiedMember<'step, 'raw>) -> Result<Self, LockFieldError> {
        if member.ordinal() != 0 {
            return Err(LockFieldError::NotSupported);
        }
        let fields = member.message().lock_fields()?;
        Ok(Self { member, fields })
    }
    pub(in crate::smb) fn original(&self) -> &VerifiedMember<'step, 'raw> {
        &self.member
    }
    pub(in crate::smb) fn fields(&self) -> LockFields {
        self.fields
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum LockFieldError {
    InvalidParameter,
    FileClosed,
    NotSupported,
    InvalidRange,
}

impl Message<'_> {
    fn lock_fields(&self) -> Result<LockFields, LockFieldError> {
        use LockFieldError::{FileClosed, InvalidParameter, InvalidRange, NotSupported};
        if self.header.command != LOCK {
            return Err(InvalidParameter);
        }
        if self.header.flags & (RELATED | REPLAY) != 0
            || !matches!(self.header.routing, Routing::Sync { .. })
            || self.raw.get(20..24) != Some(&[0; 4][..])
        {
            return Err(NotSupported);
        }
        let body = self.raw.get(64..).ok_or(InvalidParameter)?;
        let invalid = |_| InvalidParameter;
        if u16le(body, 0).map_err(invalid)? != 48 {
            return Err(InvalidParameter);
        }
        let count = usize::from(u16le(body, 2).map_err(invalid)?);
        if count == 0 || body.len() != FIXED + count * ELEMENT {
            return Err(InvalidParameter);
        }
        // The ordinary profile does not negotiate multi-channel or durable,
        // persistent or resilient OPENs. No lock-sequence replay succeeds.
        if u32le(body, 4).map_err(invalid)? >> 4 > 64 {
            return Err(InvalidParameter);
        }
        if count != 1 {
            return Err(NotSupported);
        }
        let open = WireOpenId {
            persistent: u64le(body, 8).map_err(invalid)?,
            volatile: u64le(body, 16).map_err(invalid)?,
        };
        if [open.persistent, open.volatile]
            .into_iter()
            .any(|half| matches!(half, 0 | u64::MAX))
        {
            return Err(FileClosed);
        }
        let offset = u64le(body, FIXED).map_err(invalid)?;
        let length = u64le(body, FIXED + 8).map_err(invalid)?;
        if length == 0 {
            return Err(NotSupported);
        }
        let span = Span::new(offset, length).ok_or(InvalidRange)?;
        let change = match u32le(body, FIXED + 16).map_err(invalid)? {
            0x11 => LockChange::Shared,
            0x12 => LockChange::Exclusive,
            0x04 => LockChange::Unlock,
            0x01 | 0x02 => return Err(NotSupported),
            _ => return Err(InvalidParameter),
        };
        // MS-SMB2 2.2.26.1: ignore the element's Reserved word.
        Ok(LockFields { open, span, change })
    }
}

#[cfg(test)]
#[path = "../../../tests/smb/lock_fields.rs"]
mod tests;
