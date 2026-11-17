//! SMB observations retain the exact instance, never a reconstructed DraftLocation.
use name_policy::{LookupPath, PortablePath};
use storage_contract::snapshot_worktree::{WorktreeHandle, WorktreeRevision, WorktreeStatus};
use storage_contract::NamespaceEntryKind;
use storage_contract::file_draft::UnbornWorking;
use storage_contract::snapshot_worktree::additions::{CreateWorktreeFile, WorktreeFileTarget};

pub struct SmbWorktreeLocation {
    pub status: WorktreeStatus,
    pub path: Option<LookupPath>,
}

#[derive(Clone, Debug)]
pub struct SmbWorktreeFile {
    pub target: WorktreeHandle,
    pub path: PortablePath,
    pub revision: WorktreeRevision,
    pub mtime: smb_protocol::protocol::FileMtime,
}

pub struct SmbWorktreeEntry {
    pub path: PortablePath,
    pub content: SmbWorktreeEntryContent,
}

pub enum SmbWorktreeEntryContent {
    Directory,
    File { size: u64 },
    Working { target: WorktreeFileTarget, working: UnbornWorking },
    RetainedFile {
        target: WorktreeFileTarget,
        version: storage_contract::VersionId,
        size: u64,
        mtime: smb_protocol::protocol::FileMtime,
    },
}

impl SmbWorktreeEntry {
    pub(crate) fn kind(&self) -> NamespaceEntryKind {
        match &self.content {
            SmbWorktreeEntryContent::Directory => NamespaceEntryKind::Directory,
            SmbWorktreeEntryContent::File { .. }
            | SmbWorktreeEntryContent::Working { .. }
            | SmbWorktreeEntryContent::RetainedFile { .. } => {
                NamespaceEntryKind::File
            }
        }
    }

    pub(crate) fn size(&self) -> u64 {
        match &self.content {
            SmbWorktreeEntryContent::Directory => 0,
            SmbWorktreeEntryContent::File { size }
            | SmbWorktreeEntryContent::RetainedFile { size, .. } => *size,
            SmbWorktreeEntryContent::Working { working, .. } => working.logical_size(),
        }
    }

    pub(crate) fn mtime(&self) -> smb_protocol::protocol::FileMtime {
        use smb_protocol::protocol::FileMtime;
        match &self.content {
            SmbWorktreeEntryContent::Directory => FileMtime::NotFile {},
            SmbWorktreeEntryContent::File { .. }
            | SmbWorktreeEntryContent::Working { .. } => FileMtime::Unpublished {},
            SmbWorktreeEntryContent::RetainedFile { mtime, .. } => *mtime,
        }
    }
}

pub enum SmbWorktreeWorkingSource {
    Existing(WorktreeFileTarget),
    Create(CreateWorktreeFile),
}

pub struct SmbWorktreeWorkingOpen {
    pub source: SmbWorktreeWorkingSource,
    pub writable: bool,
    pub truncate: bool,
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
