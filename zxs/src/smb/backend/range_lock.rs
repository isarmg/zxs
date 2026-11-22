//! SMB byte locks on the actual FILE and exact in-flight I/O admission.
//! No storage or session call is made while the coordinator mutex is held.
use super::{Held, Object, Slot};
use crate::smb::{
    RustSessionWitness,
    connection::Fault,
    wire::lock_fields::{LockChange, LockFields, LockRequest, Span},
};
use std::{
    io,
    sync::{Arc, Weak},
};

const LOCKS_PER_OPEN: usize = 16;
const LOCKS_PER_FILE: usize = 64;
const FILE_DATA: u32 = 0x0001 | 0x0002 | 0x0004;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::smb) enum RangeIoError {
    Conflict,
    Resources,
    Unavailable,
}

#[derive(Debug)]
pub(in crate::smb) enum LockError {
    Session(Fault),
    Original(io::Error),
    AccessDenied,
    LockNotGranted,
    RangeNotLocked,
    Resources,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Intent {
    Read,
    Write,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Mode {
    Shared,
    Exclusive,
}

struct Lock {
    span: Span,
    owner: Weak<()>,
    mode: Mode,
}

struct IoActivity {
    span: Span,
    owner: Weak<()>,
    access: Intent,
}

#[derive(Default)]
pub(super) struct Ranges {
    locks: Vec<Lock>,
    active: Vec<IoActivity>,
}

impl Ranges {
    pub(super) fn remove_owner(&mut self, owner: &Arc<()>) {
        let owner = Arc::downgrade(owner);
        self.locks.retain(|lock| !lock.owner.ptr_eq(&owner));
        self.active.retain(|io| !io.owner.ptr_eq(&owner));
    }

    fn change(&mut self, owner: Weak<()>, span: Span, change: LockChange) -> Result<(), LockError> {
        let mode = match change {
            LockChange::Shared => Mode::Shared,
            LockChange::Exclusive => Mode::Exclusive,
            LockChange::Unlock => return self.unlock(&owner, span),
        };

        // MS-FSA 2.1.4.10. A second exclusive acquisition conflicts even
        // against the same owner's exclusive lock. A shared lock blocks
        // every exclusive intent, including that of its own owner.
        let conflicts_locks = self.locks.iter().any(|lock| {
            span.overlaps(lock.span)
                && (mode == Mode::Exclusive
                    || (lock.mode == Mode::Exclusive && !lock.owner.ptr_eq(&owner)))
        });
        let conflicts_io = self.active.iter().any(|io| {
            span.overlaps(io.span)
                && match mode {
                    Mode::Shared => io.access == Intent::Write,
                    Mode::Exclusive => !io.owner.ptr_eq(&owner),
                }
        });
        if conflicts_locks || conflicts_io {
            return Err(LockError::LockNotGranted);
        }
        if self.locks.len() >= LOCKS_PER_FILE
            || self.locks.iter().filter(|lock| lock.owner.ptr_eq(&owner)).count() >= LOCKS_PER_OPEN
        {
            return Err(LockError::Resources);
        }
        self.locks.try_reserve(1).map_err(|_| LockError::Resources)?;
        self.locks.push(Lock { span, owner, mode });
        Ok(())
    }

    fn unlock(&mut self, owner: &Weak<()>, span: Span) -> Result<(), LockError> {
        // An exact shared and exclusive range can coexist on this OPEN.
        // MS-FSA 2.1.5.9 unlocks the exact exclusive match first.
        let (index, _) = self
            .locks
            .iter()
            .enumerate()
            .filter(|(_, lock)| lock.span == span && lock.owner.ptr_eq(owner))
            .max_by_key(|(_, lock)| lock.mode == Mode::Exclusive)
            .ok_or(LockError::RangeNotLocked)?;
        self.locks.swap_remove(index);
        Ok(())
    }

    fn enter(&mut self, owner: Weak<()>, span: Span, access: Intent) -> Result<(), RangeIoError> {
        if self.locks.iter().any(|lock| {
            span.overlaps(lock.span)
                && match lock.mode {
                    Mode::Exclusive => !lock.owner.ptr_eq(&owner),
                    Mode::Shared => access == Intent::Write,
                }
        }) {
            return Err(RangeIoError::Conflict);
        }
        // At most one synchronous data turn per bound OPEN. Dropping a
        // permit cannot remove a newer turn for the same OPEN.
        if self.active.iter().any(|io| io.owner.ptr_eq(&owner)) {
            return Err(RangeIoError::Unavailable);
        }
        self.active.try_reserve(1).map_err(|_| RangeIoError::Resources)?;
        self.active.push(IoActivity { span, owner, access });
        Ok(())
    }

    fn exit(&mut self, owner: &Weak<()>) {
        self.active.retain(|io| !io.owner.ptr_eq(owner));
    }
}

#[must_use]
pub(super) struct IoPermit<'a>(&'a Slot);

impl Drop for IoPermit<'_> {
    fn drop(&mut self) {
        if let Ok(mut table) = self.0.owner.table.lock()
            && let Some(row) = table.objects.get_mut(&self.0.object)
        {
            row.ranges.exit(&Arc::downgrade(&self.0.identity));
        }
    }
}

impl Slot {
    pub(super) fn range_io(
        &self,
        offset: u64,
        length: u64,
        access: Intent,
    ) -> Result<Option<IoPermit<'_>>, RangeIoError> {
        if length == 0 {
            return Ok(None); // still follows the ordinary adapter checks
        }
        let span = Span::new(offset, length).ok_or(RangeIoError::Unavailable)?;
        if !self.bound || self.released || !matches!(self.object, Object::File(..)) {
            return Err(RangeIoError::Unavailable);
        }
        let mut table = self.owner.table.lock().map_err(|_| RangeIoError::Unavailable)?;
        let row = table.objects.get_mut(&self.object).ok_or(RangeIoError::Unavailable)?;
        row.ranges.enter(Arc::downgrade(&self.identity), span, access)?;
        Ok(Some(IoPermit(self)))
    }

    pub(super) fn resize_permit(&self, old: u64, new: u64) -> Result<Option<IoPermit<'_>>, RangeIoError> {
        self.range_io(old.min(new), old.abs_diff(new), Intent::Write)
    }

    fn change_lock(&self, span: Span, change: LockChange) -> Result<(), LockError> {
        if !self.bound || self.released || self.retained || !matches!(self.object, Object::File(..)) {
            return Err(LockError::AccessDenied);
        }
        let mut table = self.owner.table.lock().map_err(|_| LockError::Unavailable)?;
        let row = table.objects.get_mut(&self.object).ok_or(LockError::Unavailable)?;
        row.ranges.change(Arc::downgrade(&self.identity), span, change)
    }
}

/// An exact mutation of the original OPEN's in-memory locks, not a wire ID
/// or new authority. The same synchronous lane cannot retask it before send.
#[must_use]
pub(in crate::smb) struct PreparedLock {
    owner: Weak<()>,
    object: Object,
    request: LockFields,
    scope: RustSessionWitness,
}

impl PreparedLock {
    pub(in crate::smb) fn matches(&self, request: LockFields) -> bool {
        self.request == request
    }

    pub(super) fn require(
        &self,
        slot: &Slot,
        current: impl FnOnce(&RustSessionWitness) -> io::Result<()>,
    ) -> io::Result<()> {
        if !self.scope.is_lock()
            || !slot.bound
            || slot.released
            || slot.retained
            || self.object != slot.object
            || !self.owner.ptr_eq(&Arc::downgrade(&slot.identity))
        {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        current(&self.scope)
    }
}

impl<H> Held<H> {
    pub(super) fn change_lock(
        &self,
        request: &LockRequest<'_, '_>,
        current: impl FnOnce(&RustSessionWitness) -> io::Result<()>,
    ) -> Result<PreparedLock, LockError> {
        let scope = request.original().rust_session_witness().map_err(LockError::Session)?;
        let request = request.fields();
        if !scope.is_lock() || self.grant.0 & FILE_DATA == 0 {
            return Err(LockError::AccessDenied);
        }
        current(&scope).map_err(LockError::Original)?;
        let Held { _slot: slot, .. } = self;
        slot.change_lock(request.span(), request.change())?;
        Ok(PreparedLock {
            owner: Arc::downgrade(&slot.identity),
            object: slot.object,
            request,
            scope,
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/smb/range_lock.rs"]
pub(super) mod tests;
