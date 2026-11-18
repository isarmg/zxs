//! Connection-owned native work. Only the actual core backend can create its handle.
use name_policy::LookupPath;
use storage_contract::file_draft::DraftLocation;
use storage_contract::snapshot_worktree::additions::WorktreeFileTarget;

pub struct SmbNativeOpen<'a> {
    pub branch: &'a str,
    pub path: &'a LookupPath,
    pub writable: bool,
    pub create: bool,
    pub truncate: bool,
    pub exclusive: bool,
}

pub struct SmbNativeHandle {
    pub(crate) source: NativeSource,
}

pub(crate) enum NativeSource {
    Current {
        target: DraftLocation,
        version: Option<storage_contract::VersionId>,
        mtime: smb_protocol::protocol::FileMtime,
        #[cfg(feature = "core")]
        held: storage_engine::NativeWorkingFile,
    },
    Worktree {
        visible: storage_contract::OperationId,
        target: WorktreeFileTarget,
        #[cfg(feature = "core")]
        held: storage_engine::NativeWorkingFile,
    },
}

impl SmbNativeHandle {
    pub(crate) fn entry(&self) -> storage_contract::EntryId {
        match &self.source {
            NativeSource::Current { target, .. } => target.entry,
            NativeSource::Worktree { target, .. } => target.entry,
        }
    }

    pub(crate) fn path(&self) -> &name_policy::PortablePath {
        match &self.source {
            NativeSource::Current { target, .. } => &target.path,
            NativeSource::Worktree { target, .. } => &target.path,
        }
    }

    pub(crate) fn version(&self) -> Option<storage_contract::VersionId> {
        match &self.source {
            NativeSource::Current { version, .. } => *version,
            NativeSource::Worktree { .. } => None,
        }
    }

    pub(crate) fn mtime(&self) -> smb_protocol::protocol::FileMtime {
        match &self.source {
            NativeSource::Current { mtime, .. } => *mtime,
            NativeSource::Worktree { .. } => smb_protocol::protocol::FileMtime::Unpublished {},
        }
    }

    pub(crate) fn operation(&self) -> Option<storage_contract::OperationId> {
        #[cfg(feature = "core")]
        {
            match &self.source {
                NativeSource::Current { held, .. } | NativeSource::Worktree { held, .. } => {
                    held.operation()
                }
            }
        }
        #[cfg(not(feature = "core"))]
        { None }
    }
}

pub enum SmbNativeEdit<'a> {
    Write { offset: u64, bytes: &'a [u8] },
    Append { bytes: &'a [u8] },
    Resize { length: u64 },
}
