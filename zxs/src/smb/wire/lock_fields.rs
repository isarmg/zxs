//! One standalone nonblocking LOCK request on one OPEN. Decoding grants no access.
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
const UNLOCK: u32 = 0x04;
const FAIL_IMMEDIATELY: u32 = 0x10;

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
pub(in crate::smb) struct LockFields<'raw> {
    open: WireOpenId,
    elements: &'raw [[u8; ELEMENT]],
}

impl<'raw> LockFields<'raw> {
    pub(in crate::smb) fn open(self) -> WireOpenId {
        self.open
    }

    pub(in crate::smb) fn unlocking(self) -> bool {
        self.elements.first().is_some_and(|raw| flags(raw) & UNLOCK != 0)
    }

    pub(in crate::smb) fn ordered(
        self,
    ) -> impl Iterator<Item = Result<(Span, LockChange), LockFieldError>> + 'raw {
        let unlocking = self.unlocking();
        self.elements.iter().map(move |raw| element(raw, unlocking))
    }
}

#[must_use]
pub(in crate::smb) struct LockRequest<'step, 'raw> {
    member: VerifiedMember<'step, 'raw>,
    fields: LockFields<'raw>,
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
    pub(in crate::smb) fn fields(&self) -> LockFields<'raw> {
        self.fields
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum LockFieldError {
    InvalidParameter,
    FileClosed,
    NotSupported,
    InvalidRange,
    UnexpectedUnlock,
}

impl<'raw> Message<'raw> {
    fn lock_fields(&self) -> Result<LockFields<'raw>, LockFieldError> {
        use LockFieldError::{FileClosed, InvalidParameter, NotSupported};
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
        let (elements, tail) = body.get(FIXED..).ok_or(InvalidParameter)?.as_chunks::<ELEMENT>();
        if count == 0 || elements.len() != count || !tail.is_empty() {
            return Err(InvalidParameter);
        }
        // The ordinary profile does not negotiate multi-channel or durable,
        // persistent or resilient OPENs. No lock-sequence replay succeeds.
        if u32le(body, 4).map_err(invalid)? >> 4 > 64 {
            return Err(InvalidParameter);
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
        let fields = LockFields { open, elements };
        let unlocking = fields.unlocking();
        if let [raw] = elements {
            // Preserve the singleton's field-error check before OPEN lookup.
            element(raw, unlocking)?;
        } else if !unlocking && elements.iter().any(|raw| flags(raw) & FAIL_IMMEDIATELY == 0) {
            return Err(InvalidParameter);
        }
        Ok(fields)
    }
}

fn flags(raw: &[u8; ELEMENT]) -> u32 {
    u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]])
}

fn element(raw: &[u8; ELEMENT], unlocking: bool) -> Result<(Span, LockChange), LockFieldError> {
    use LockFieldError::{InvalidParameter, InvalidRange, NotSupported, UnexpectedUnlock};
    let flags = flags(raw);
    if unlocking && !matches!(flags, 0x04 | 0x14) {
        return Err(InvalidParameter);
    }
    if !unlocking && flags & UNLOCK != 0 {
        return Err(UnexpectedUnlock);
    }
    let invalid = |_| InvalidParameter;
    let length = u64le(raw, 8).map_err(invalid)?;
    if length == 0 {
        return Err(NotSupported);
    }
    let span = Span::new(u64le(raw, 0).map_err(invalid)?, length).ok_or(InvalidRange)?;
    let change = match (unlocking, flags) {
        (true, 0x04 | 0x14) => LockChange::Unlock, // the immediate bit may be ignored for unlock
        (false, 0x11) => LockChange::Shared,
        (false, 0x12) => LockChange::Exclusive,
        (false, 0x01 | 0x02) => return Err(NotSupported),
        _ => return Err(InvalidParameter),
    };
    // MS-SMB2 2.2.26.1: ignore the element's Reserved word.
    Ok((span, change))
}

#[cfg(test)]
#[path = "../../../tests/smb/lock_fields.rs"]
pub(in crate::smb) mod tests;
