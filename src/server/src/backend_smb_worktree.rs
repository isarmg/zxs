//! SMB observations retain the exact instance, never a reconstructed DraftLocation.
use name_policy::{LookupPath, PortablePath};
use storage_contract::snapshot_worktree::{WorktreeHandle, WorktreeRevision, WorktreeStatus};
use storage_contract::NamespaceEntryKind;

pub struct SmbWorktreeLocation {
    pub status: WorktreeStatus,
    pub path: Option<LookupPath>,
}

#[derive(Clone, Debug)]
pub struct SmbWorktreeFile {
    pub target: WorktreeHandle,
    pub path: PortablePath,
    pub revision: WorktreeRevision,
}

pub struct SmbWorktreeEntry {
    pub path: PortablePath,
    pub kind: NamespaceEntryKind,
    pub size: u64,
}

#[derive(Clone)]
pub struct SmbWorktreeCursor {
    #[cfg(feature = "core")]
    pub(crate) held: storage_engine::WorktreeCursor,
}

pub struct SmbWorktreePage {
    #[cfg(feature = "core")]
    pub(crate) stamp: storage_engine::WorktreePageStamp,
    pub entries: Vec<SmbWorktreeEntry>,
    pub next: Option<SmbWorktreeCursor>,
}

pub enum SmbWorktreeAccess<'a> {
    Read,
    Write { path: &'a PortablePath, expected: WorktreeRevision },
}
