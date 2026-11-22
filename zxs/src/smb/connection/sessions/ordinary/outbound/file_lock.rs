//! Locks belong only to the bound, current ordinary data OPEN.
use super::{CheckedBatch, Connection, Fault, Payload, Result, SyncReply, UnsignedSmall, current};
use super::{super::opens::Opened, VerifiedMember, root_io::RequestSlot};
use crate::smb::{
    backend::{LockError, PreparedLock},
    wire::{
        fixed_reply::LockFailure,
        io_fields::WireOpenId,
        lock_fields::{LOCK, LockFieldError, LockRequest},
        message::{Message, Routing},
    },
};
use std::{io, num::NonZeroU64};

impl CheckedBatch<'_, '_> {
    pub(in crate::smb) fn try_send_file_lock(&mut self) -> Result<bool> {
        self.before_reply()?;
        let [message] = self.original.frame.messages() else {
            return Ok(false);
        };
        if message.header.command != LOCK {
            return Ok(false);
        }
        let Routing::Sync { tree: tid } = message.header.routing else {
            return Err(Fault::WrongReply);
        };
        if self.original.payload(0) != Some(Payload::NoVariableRule) {
            return Err(Fault::WrongReply);
        }
        let sid = self.ids.at(0).ok_or(Fault::SessionState)?;
        let original = RequestSlot { sid, tree: tid, mid: message.header.message };
        let (draft, pending) = {
            let (session, trees, opens) = self.owner.member_tables(sid, &self.revision)?;
            let Some(tree) = trees.root(tid) else {
                return self.lock_refusal(message, sid, LockFailure::NetworkNameDeleted);
            };
            let request = match LockRequest::parse(VerifiedMember { message, ordinal: 0, session }) {
                Ok(request) => request,
                Err(error) => return self.lock_refusal(message, sid, field_failure(error)),
            };
            let wire = request.fields().open();
            let Some(slot) = opens.find(wire).filter(|slot| slot.tree() == tid) else {
                return self.lock_refusal(message, sid, LockFailure::FileClosed);
            };
            let result = match slot.object() {
                Opened::File(file) => file.change_lock(tree, &request),
                Opened::Writer(file) => file.change_lock(tree, &request),
                Opened::Directory(_) | Opened::Root(_) => {
                    return self.lock_refusal(message, sid, LockFailure::InvalidParameter);
                }
                Opened::Retained(file) => {
                    let status = if file.is_directory() {
                        LockFailure::InvalidParameter
                    } else {
                        LockFailure::NotSupported
                    };
                    return self.lock_refusal(message, sid, status);
                }
            };
            let proof = match result {
                Ok(proof) => proof,
                Err(error) => return self.lock_refusal(message, sid, lock_failure(error)?),
            };
            let pending = PendingLock { original, wire, proof };
            let draft = UnsignedSmall::locked(&request, &pending).map_err(|_| Fault::WrongReply)?;
            (draft, pending)
        };
        self.next = 1;
        self.small_replies(std::iter::once(draft))?.send_locked(pending)?;
        Ok(true)
    }

    fn lock_refusal(
        &mut self,
        message: &Message<'_>,
        sid: NonZeroU64,
        why: LockFailure,
    ) -> Result<bool> {
        self.owner.member_scope(sid, &self.revision)?;
        let original = LockRefusal { message, sid, why };
        let draft = UnsignedSmall::lock_refusal(&original).map_err(|_| Fault::WrongReply)?;
        self.next = 1;
        self.small_replies(std::iter::once(draft))?.send()?;
        Ok(true)
    }
}

fn field_failure(error: LockFieldError) -> LockFailure {
    match error {
        LockFieldError::InvalidParameter => LockFailure::InvalidParameter,
        LockFieldError::FileClosed => LockFailure::FileClosed,
        LockFieldError::NotSupported => LockFailure::NotSupported,
        LockFieldError::InvalidRange => LockFailure::InvalidRange,
    }
}

fn lock_failure(error: LockError) -> Result<LockFailure> {
    Ok(match error {
        LockError::Session(cause) => return Err(cause),
        LockError::Original(cause) => match cause.kind() {
            io::ErrorKind::PermissionDenied => LockFailure::AccessDenied,
            io::ErrorKind::NotFound | io::ErrorKind::WouldBlock => LockFailure::FileNotAvailable,
            io::ErrorKind::NotConnected => LockFailure::VolumeDismounted,
            io::ErrorKind::ReadOnlyFilesystem => LockFailure::MediaWriteProtected,
            io::ErrorKind::OutOfMemory => LockFailure::Resources,
            _ => LockFailure::ServerUnavailable,
        },
        LockError::AccessDenied => LockFailure::AccessDenied,
        LockError::LockNotGranted => LockFailure::LockNotGranted,
        LockError::RangeNotLocked => LockFailure::RangeNotLocked,
        LockError::Resources => LockFailure::Resources,
        LockError::Unavailable => LockFailure::ServerUnavailable,
    })
}

pub(in crate::smb) struct LockRefusal<'a, 'raw> {
    message: &'a Message<'raw>,
    sid: NonZeroU64,
    why: LockFailure,
}

impl<'a, 'raw> LockRefusal<'a, 'raw> {
    pub(in crate::smb) fn fields(&self) -> (&'a Message<'raw>, NonZeroU64, LockFailure) {
        (self.message, self.sid, self.why)
    }
}

#[must_use]
pub(in crate::smb) struct PendingLock {
    original: RequestSlot,
    wire: WireOpenId,
    proof: PreparedLock,
}

impl PendingLock {
    pub(in crate::smb) fn sid(&self) -> NonZeroU64 {
        self.original.sid
    }

    pub(in crate::smb) fn matches(&self, request: &LockRequest<'_, '_>) -> bool {
        self.original.matches(request.original(), self.sid())
            && self.wire == request.fields().open()
            && self.proof.matches(request.fields())
    }

    fn check(&self, connection: &Connection) -> Result<()> {
        let tree = self.original.root(connection)?;
        let slot = connection
            .open_table(self.sid())?
            .find(self.wire)
            .filter(|slot| slot.tree() == self.original.tree)
            .ok_or(Fault::SessionState)?;
        match slot.object() {
            Opened::File(file) => file.require_lock(tree, &self.proof),
            Opened::Writer(file) => file.require_lock(tree, &self.proof),
            _ => return Err(Fault::SessionState),
        }
        .map_err(|_| Fault::SessionState)
    }
}

impl SyncReply<'_> {
    fn send_locked(self, pending: PendingLock) -> Result<()> {
        // The LOCK/UNLOCK changed volatile state on this original OPEN.
        // Losing the reply closes the connection and retires its OPENs.
        pending.check(self.tx.plan.owner)?;
        let SyncReply { tx, ids, revision, count } = self.write_original()?.0;
        tx.publish_with(move |connection| {
            current(connection, ids, revision, count)?;
            pending.check(connection)
        })
    }
}
