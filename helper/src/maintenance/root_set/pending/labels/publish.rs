//! Only this private step publishes a new empty registration. No operator
//! permission or public CLI is supplied. A cold query never calls confirmation.
use super::{PendingRootSet, RegisterName, SyncedLabels, WorkerPolicy};
use super::super::{Root, RootHolds, Selection, acquire, inspect_volumes};
use crate::{maintenance::pending::ColdEmptyReady, pinned::invalid};
use rustix::fs::{RenameFlags, renameat_with};
use std::{fmt, io, os::unix::ffi::OsStrExt};

enum PublishError {
    NotAttempted(io::Error),
    // From entry to rename onward, the caller cannot infer failure/commit from
    // an error. Keep all names and labels; inspect the same original operation.
    InspectOriginal(io::Error),
}
impl fmt::Debug for PublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (phase, cause) = match self {
            Self::NotAttempted(cause) => ("NotAttempted", cause),
            Self::InspectOriginal(cause) => ("InspectOriginal", cause),
        };
        f.debug_tuple(phase).field(cause).finish()
    }
}
#[must_use]
struct AwaitingParentSync<'r>(PendingRootSet<'r>);
#[must_use]
struct ConfirmedEmpty<'r> { source: ColdEmptyReady, roots: RootHolds<'r> }

impl<'r> SyncedLabels<'r> {
    fn publish(self, op: [u8;16], worker: &WorkerPolicy) -> Result<ConfirmedEmpty<'r>, PublishError> {
        self.rename_original(op, worker)?.confirm().map_err(PublishError::InspectOriginal)
    }

    fn rename_original(self, op: [u8;16], worker: &WorkerPolicy) -> Result<AwaitingParentSync<'r>, PublishError> {
        if !self.0.source.is_original_op(op) {
            return Err(PublishError::NotAttempted(invalid("only the original initialization operation may publish")));
        }
        worker.require(self.0.source.storage_uid()).map_err(PublishError::NotAttempted)?;
        self.recheck().map_err(PublishError::NotAttempted)?; // ends with the real named source and register FD
        let system = &self.0.roots.all()[0].dir;
        // NOREPLACE protects the destination. Linux has no source-inode CAS:
        // controlled writers of this protected SYSTEM must all obey the held
        // register and volume locks. Recheck the same original after rename;
        // an untrusted same-UID writer is not isolated by advisory flock.
        renameat_with(system, RegisterName::Pending.entry(), system, RegisterName::Ready.entry(),
            RenameFlags::NOREPLACE).map_err(|e| PublishError::InspectOriginal(e.into()))?;
        Ok(AwaitingParentSync(self.0))
    }
}

impl<'r> AwaitingParentSync<'r> {
    fn confirm(self) -> io::Result<ConfirmedEmpty<'r>> {
        let PendingRootSet { source, roots, .. } = self.0;
        roots.all()[0].dir.sync_all()?; // the actual parent of the rename; no alternate directory
        let source = ColdEmptyReady::read_renamed(source)?; // same register EX, even after a cold resume
        check_all(&source, roots.all())?;
        Ok(ConfirmedEmpty { source, roots })
    }
}

fn check_all(source: &ColdEmptyReady, roots: &[Root]) -> io::Result<()> {
    let g = source.original_image()?; // original bytes, zero frontier, actual SYSTEM digest
    let uid = source.storage_uid();
    if !g.has_root_count(uid, roots.len()) || !source.same_system(&roots[0].observed)
        || roots.iter().enumerate().any(|(i,r)| !g.matches_root_origin(i, r.name.path().as_os_str().as_bytes(),
            r.component_limit, r.observed.fields())) {
        return Err(invalid("published register and locked volume set do not name the same originals"));
    }
    let labels = inspect_volumes(roots, uid, &g, RegisterName::Ready)?;
    if labels.iter().any(|set| !set) { return Err(invalid("published empty registration has an unlabelled original volume")); }
    source.recheck() // after all volumes, still the same sole name, native inode and frontier
}

impl ConfirmedEmpty<'_> {
    fn recheck(&self) -> io::Result<()> { check_all(&self.source, self.roots.all()) }
}

impl ConfirmedEmpty<'static> {
    /// Explicit same-operation confirmation, not passive status. An earlier rename
    /// could be visible although its parent sync failed or the response was lost.
    /// Never rename again, create a label, infer a different operation, or repair.
    fn confirm_existing(source: ColdEmptyReady, selected: &Selection, op: [u8;16],
                        worker: &WorkerPolicy) -> io::Result<Self> {
        let uid = source.storage_uid();
        worker.require(uid)?;
        let g = source.original_image()?;
        if !g.has_init_operation(&op) { return Err(invalid("not the published initialization operation")); }
        // acquire matches all caller-selected paths and original physical rows;
        // it does not use the registration as a source of unsolicited targets.
        let roots = acquire(selected, uid, Some(&g))?;
        check_all(&source, &roots)?; // fail on even the last bad row, before any sync
        // No prior process's synchronization is inferred from the presence
        // of labels. Retain all locks and sync the exact originals, SYSTEM last.
        for r in roots[1..].iter().chain(roots[..1].iter()) {
            worker.require(uid)?;
            r.checked_name(uid)?;
            r.dir.sync_all()?;
        }
        check_all(&source, &roots)?;
        Ok(Self { source, roots: RootHolds::Cold(roots) })
    }
}

#[cfg(test)]
#[path = "../../../../../tests/maintenance/publish_empty.rs"]
mod tests;
