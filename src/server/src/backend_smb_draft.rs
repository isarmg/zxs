//! Request-local manual FILE selection. No value here grants publication or
//! recovery: every use re-enters the backend's current admission lock.
use storage_contract::file_draft::{DraftBase, DraftLocation, FileWorkingState, WorkingRevision};

#[derive(Clone, Debug)]
pub struct SmbManualFile {
    pub(crate) target: DraftLocation,
    pub(crate) base: DraftBase,
    pub(crate) working: FileWorkingState,
}

impl SmbManualFile {
    pub(crate) fn revision(&self) -> WorkingRevision {
        self.working.revision()
    }
}

/// The exact committed FILE behind an automatic descriptor generation.
#[derive(Clone, Debug)]
pub struct SmbAutomaticFile {
    pub(crate) target: DraftLocation,
    pub(crate) base: DraftBase,
    pub(crate) revision: WorkingRevision,
}

#[derive(Clone, Copy)]
pub enum SmbDraftAccess {
    Read,
    Write { expected: WorkingRevision },
}
