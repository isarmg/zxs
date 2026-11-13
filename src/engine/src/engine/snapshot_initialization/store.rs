//! Fresh-only intent bytes, not a decision, registration or publication authority.
//! The caller owns root authorization and the operation's writer fence. Unknown
//! entries are preserved; even an identical retry never replaces the named file.
use super::intent::{AdmittedIntent, IntentError, MAX_INTENT_BYTES};
use crate::disk::{DirectoryIoError, DiskRoot, read_file_bytes};
use crate::persist::layout::snapshot_init;
use crate::ReadFile;
use platform_io::fs_safety::linux::FileIdentity;
use std::fs::Metadata;
use std::os::unix::fs::{FileExt, MetadataExt};

#[derive(Debug, error_definition::Error)]
pub(super) enum StoreError {
    #[error("snapshot initialization intent disk access failed")]
    Disk(#[source] crate::Error),
    #[error("snapshot initialization intent root access failed")]
    Directory(#[source] DirectoryIoError),
    #[error("snapshot initialization intent file access failed")]
    Io(#[source] std::io::Error),
    #[error("snapshot initialization intent admission failed")]
    Intent(#[source] IntentError),
    #[error("snapshot initialization intent is not a single-link regular file")]
    FileKind,
    #[error("snapshot initialization intent bytes are not canonical")]
    Canonical,
    #[error("another original intent occupies the initialization slot")]
    Occupied,
    #[error("snapshot initialization intent disappeared or changed while held")]
    Changed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    size: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    links: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl Stamp {
    fn capture(metadata: Metadata) -> Result<Self, StoreError> {
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(StoreError::FileKind);
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            mode: metadata.mode(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            links: metadata.nlink(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

struct Observed {
    intent: AdmittedIntent,
    file: ReadFile,
    stamp: Stamp,
    root_identity: FileIdentity,
}
impl Observed {
    fn capture(root: &DiskRoot) -> Result<Option<Self>, StoreError> {
        let root_identity = root.held_identity().map_err(StoreError::Directory)?;
        let Some(file) = root.open_optional_read_entry(&snapshot_init::INTENT)
            .map_err(StoreError::Disk)?
        else {
            return Ok(None);
        };
        let stamp = Stamp::capture(file.metadata().map_err(StoreError::Io)?)?;
        let bytes = read_file_bytes(
            file.try_clone().map_err(StoreError::Io)?, MAX_INTENT_BYTES,
        ).map_err(StoreError::Disk)?;
        let intent = AdmittedIntent::decode(&bytes).map_err(StoreError::Intent)?;
        if intent.bytes() != bytes.as_slice() {
            return Err(StoreError::Canonical);
        }
        let observed = Self { intent, file, stamp, root_identity };
        observed.check(root)?;
        Ok(Some(observed))
    }

    fn check(&self, root: &DiskRoot) -> Result<(), StoreError> {
        if root.held_identity().map_err(StoreError::Directory)? != self.root_identity {
            return Err(StoreError::Changed);
        }
        verify(&self.file, self.stamp, self.intent.bytes())?;
        let named = root.open_optional_read_entry(&snapshot_init::INTENT)
            .map_err(StoreError::Disk)?.ok_or(StoreError::Changed)?;
        verify(&named, self.stamp, self.intent.bytes())?;
        if Stamp::capture(self.file.metadata().map_err(StoreError::Io)?)? != self.stamp
            || root.held_identity().map_err(StoreError::Directory)? != self.root_identity
        {
            return Err(StoreError::Changed);
        }
        Ok(())
    }

    fn synchronize(&self, root: &DiskRoot) -> Result<(), StoreError> {
        self.check(root)?;
        self.file.sync_all().map_err(StoreError::Io)?;
        self.check(root)?;
        root.sync_dir(root.path()).map_err(StoreError::Disk)?;
        self.check(root)
    }
}

fn verify(file: &ReadFile, stamp: Stamp, bytes: &[u8]) -> Result<(), StoreError> {
    if Stamp::capture(file.metadata().map_err(StoreError::Io)?)? != stamp
        || stamp.size != bytes.len() as u64
    {
        return Err(StoreError::Changed);
    }
    // Positional reads preserve the retained descriptor's cursor and recheck
    // the original bytes, not merely a new self-consistent decoded record.
    let mut buffer = [0u8; 64 * 1024];
    let mut offset = 0;
    for chunk in bytes.chunks(buffer.len()) {
        file.read_exact_at(&mut buffer[..chunk.len()], offset).map_err(StoreError::Io)?;
        if &buffer[..chunk.len()] != chunk {
            return Err(StoreError::Changed);
        }
        offset += chunk.len() as u64;
    }
    if Stamp::capture(file.metadata().map_err(StoreError::Io)?)? != stamp {
        return Err(StoreError::Changed);
    }
    file.check_running().map_err(StoreError::Disk)
}

pub(super) fn read(root: &DiskRoot) -> Result<Option<AdmittedIntent>, StoreError> {
    let _activity = root.retain_io_activity().map_err(StoreError::Disk)?;
    Ok(Observed::capture(root)?.map(|observed| observed.intent))
}

pub(super) fn save(root: &DiskRoot, intent: &AdmittedIntent) -> Result<(), StoreError> {
    let _activity = root.retain_io_activity().map_err(StoreError::Disk)?;
    if let Some(existing) = Observed::capture(root)? {
        // Canonical byte equality binds every original field, including the
        // operation and actor. Matching an operation ID alone is insufficient.
        if existing.intent.bytes() != intent.bytes() {
            return Err(StoreError::Occupied);
        }
        return existing.synchronize(root);
    }
    root.write_relative_atomic(&snapshot_init::INTENT.as_relative(), intent.bytes(), false)
        .map_err(StoreError::Disk)?;
    let installed = Observed::capture(root)?.ok_or(StoreError::Changed)?;
    if installed.intent.bytes() != intent.bytes() {
        return Err(StoreError::Changed);
    }
    installed.synchronize(root)
}
