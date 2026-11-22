//! The first durable name, not a volume claim or a completed initialization.
//! Deliberately module-private: no production caller until there is independent
//! operator approval bound to the full request, not merely the storage UID or EX.
use super::{FreshRootLocks, SyncedUnlinked};
use crate::{maintenance::{pending::ColdPending, thread_evidence::WorkerPolicy}, pinned::invalid};
use rustix::fs::{AtFlags, linkat};
use std::{fmt, fs::File, io};

/// This type is nameable by the common reader, but can ONLY be constructed here
/// after linking and syncing our own original. No external File is accepted,
/// there is no public producer and the owning file is never extracted again.
pub(in crate::maintenance) struct FirstLinkedRegister(File);
impl FirstLinkedRegister {
    pub(in crate::maintenance) fn original(&self) -> &File { &self.0 }
}

enum NameError {
    NotAttempted(io::Error),
    // Fault after attempting link (including an ambiguous return or parent
    // fsync): do not clean, allocate another ID, or automatically retry.
    // The caller may only inspect the originally selected pool and init_op.
    InspectOriginal(io::Error),
}
impl fmt::Debug for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (phase, cause) = match self {
            Self::NotAttempted(cause) => ("NotAttempted", cause),
            Self::InspectOriginal(cause) => ("InspectOriginal", cause),
        };
        f.debug_tuple(phase).field(cause).finish()
    }
}

#[must_use]
struct Linked<'r> {
    roots: &'r FreshRootLocks,
    system: &'r File,
    register: File,
    image: Vec<u8>,
    uid: u32,
    op: [u8; 16],
}

#[must_use]
pub(in crate::maintenance) struct FirstPending<'r> { roots: &'r FreshRootLocks, source: ColdPending }
impl<'r> FirstPending<'r> {
    // Both origin guards move together. No raw descriptor or partial selection is lent.
    pub(in crate::maintenance) fn into_guards(self) -> (&'r FreshRootLocks, ColdPending) { (self.roots, self.source) }

    fn recheck(&self) -> io::Result<()> { self.roots.check_just_linked(&self.source) }
}

impl<'r> SyncedUnlinked<'r> {
    fn name_initializing(self, op: [u8; 16], worker: &WorkerPolicy) -> Result<FirstPending<'r>, NameError> {
        self.link_first(op, worker)?.confirm().map_err(NameError::InspectOriginal)
    }

    fn link_first(self, op: [u8; 16], worker: &WorkerPolicy) -> Result<Linked<'r>, NameError> {
        if !self.bytes.checked().map_err(|_| NameError::NotAttempted(invalid("invalid initial image")))?.has_init_operation(&op) {
            return Err(NameError::NotAttempted(invalid("only the original initialization request may be named")));
        }
        self.recheck().map_err(NameError::NotAttempted)?;
        let register = self.register;
        worker.require(register.storage_uid).map_err(NameError::NotAttempted)?;

        // Actual kernel check with the same description and creator; no shell,
        // /proc source name, fallback, injected handle, overwrite or privilege change.
        linkat(&register.held, c"", register.system, c"INITIALIZING", AtFlags::EMPTY_PATH)
            .map_err(|e| NameError::InspectOriginal(e.into()))?;
        Ok(Linked { roots: self.roots, system: register.system, register: register.held,
            image: self.bytes.image, uid: register.storage_uid, op })
    }
}

impl<'r> Linked<'r> {
    fn confirm(self) -> io::Result<FirstPending<'r>> {
        self.system.sync_all()?; // parent of the actual link, even if the selected spelling moved
        let source = ColdPending::from_first_name(self.roots.selected_system(), self.uid,
            self.op, FirstLinkedRegister(self.register), &self.image)?; // move the same EX; no independent reopen/unlock
        let pending = FirstPending { roots: self.roots, source };
        pending.recheck()?; // original group, including its original SYSTEM mount; all labels absent
        Ok(pending)
    }
}

// Other isolated source controls need the real warm transition, not a fabricated
// receipt. This test-only visibility grants no production command.
#[cfg(test)]
pub(in crate::maintenance) fn fixture_first<'r>(stage: SyncedUnlinked<'r>, op: [u8; 16]) -> FirstPending<'r> {
    stage.name_initializing(op, &WorkerPolicy::fixture()).expect("qualified worker and new original required")
}

#[cfg(test)]
#[path = "../../../../tests/maintenance/first_name.rs"]
mod tests;
