//! Filesystem operations whose names encode their durability and link-safety guarantees.

#[cfg(test)]
use std::fs;
use std::{
    ffi::OsStr,
    fs::File,
    io,
    path::{Component, Path, PathBuf},
};

mod unix;

/// Whether a private payload must be newly created or already exist.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PayloadOpenMode {
    /// Exclusively create a new payload, failing if its name already exists.
    CreateNew,
    /// Open an existing payload without creating or truncating it.
    Existing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelativePath(PathBuf);

/// Exactly one canonical directory entry, not a relative traversal path.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EntryName(PathBuf);
impl EntryName {
    pub fn new(name: impl AsRef<OsStr>) -> Result<Self, Error> {
        let path = RelativePath::new(Path::new(name.as_ref()))?;
        if path.as_path().components().count() != 1 {
            return Err(Error::UnsafeRelativePath(path.0));
        }
        Ok(Self(path.0))
    }
    pub fn as_path(&self) -> &Path {
        &self.0
    }
    pub fn as_os_str(&self) -> &OsStr {
        self.0.as_os_str()
    }
    pub fn as_relative(&self) -> RelativePath {
        RelativePath(self.0.clone())
    }
}

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub name: EntryName,
    pub bytes: u64,
}

impl RelativePath {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        if path.as_os_str().is_empty()
            || path.as_os_str().as_encoded_bytes().contains(&0)
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(Error::UnsafeRelativePath(path.to_path_buf()));
        }
        let canonical: PathBuf = path.components().collect();
        if canonical.as_os_str() != path.as_os_str() {
            return Err(Error::UnsafeRelativePath(path.to_path_buf()));
        }
        Ok(Self(path.to_path_buf()))
    }
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

#[derive(Debug)]
pub struct PrivateDirectory {
    path: PathBuf,
    directory: File,
    administration_owner: Option<(u32, u32)>,
}

impl PrivateDirectory {
    /// Create missing private components and reject replacement-capable ancestry.
    /// Root-owned standard ancestors and trusted sticky directories are allowed;
    /// existing directories are validated, never chmodded or re-owned.
    pub fn create_trusted(path: impl AsRef<Path>) -> Result<Self, Error> {
        unix::trusted_private_directory(path.as_ref(), true)
    }
    pub fn open_trusted(path: impl AsRef<Path>) -> Result<Self, Error> {
        unix::trusted_private_directory(path.as_ref(), false)
    }
    /// Open a private single-link payload for reading and writing, without
    /// truncation, relative to this held directory. Filesystem failures retain
    /// their I/O source; unsafe file types, links and permissions are rejected.
    pub fn open_payload(&self, name: &EntryName, mode: PayloadOpenMode) -> Result<File, Error> {
        unix::open_payload(self, name, mode)
    }
    /// Detect replacement without ever using the replacement as the payload.
    pub fn verify_payload(&self, name: &EntryName, held: &File) -> Result<(), Error> {
        use std::os::unix::fs::MetadataExt;
        self.verify_identity()?;
        let named = self
            .open_payload(name, PayloadOpenMode::Existing)?
            .metadata()?;
        let held = held.metadata()?;
        if named.dev() != held.dev() || named.ino() != held.ino() {
            return Err(Error::IdentityChanged(self.path.join(name.as_path())));
        }
        Ok(())
    }
    /// Open an existing service-owned private directory for authorized offline
    /// administration. Unix requires effective uid 0 or the actual owner and
    /// exact 0700 permissions. Newly created files keep that owner's uid/gid;
    /// existing files and directories are never repaired. Runtime callers must
    /// use [`Self::open_existing`].
    pub fn open_for_administration(path: impl AsRef<Path>) -> Result<Self, Error> {
        unix::open_administrative_directory(path.as_ref())
    }
    /// Create or validate a direct child of this held private directory.
    pub fn create_child(&self, name: &EntryName) -> Result<Self, Error> {
        unix::create_private_child(self, name)
    }

    /// Open an existing private directory without creating or changing anything.
    pub fn open_existing(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        unix::open_private_directory(path)
    }

    /// Enumerate only regular, single-linked children with hard entry/byte budgets.
    pub fn files(&self, limits: InventoryLimits) -> Result<Vec<FileEntry>, Error> {
        unix::files(self, limits)
    }
    pub fn read_bounded(&self, name: &EntryName, max_bytes: usize) -> Result<Vec<u8>, Error> {
        unix::read_bounded(self, name, max_bytes)
    }
    pub fn remove_file(&self, name: &EntryName) -> Result<(), Error> {
        unix::remove_file(self, name)
    }
    pub fn create(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        unix::create_private_directory(path)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Held directory descriptor for ZXS's streaming payload publisher.
    /// Callers never reconstruct an untrusted absolute payload path.
    pub(crate) fn directory_handle(&self) -> &File {
        &self.directory
    }
    /// Confirm that the named directory still refers to this held handle.
    pub fn verify_identity(&self) -> Result<(), Error> {
        let named = std::fs::symlink_metadata(&self.path)?;
        let held = self.directory.metadata()?;
        if !named.is_dir() || named.file_type().is_symlink() {
            return Err(Error::UnsafeFileType(self.path.clone()));
        }
        {
            use std::os::unix::fs::MetadataExt;
            if named.dev() != held.dev() || named.ino() != held.ino() {
                return Err(Error::UnsafeFileType(self.path.clone()));
            }
            let owner = self
                .administration_owner
                .map_or_else(|| rustix::process::geteuid().as_raw(), |(uid, _)| uid);
            if named.uid() != owner || named.mode() & 0o7777 != 0o700 {
                return Err(Error::UnsafePermissions(self.path.clone()));
            }
        }
        Ok(())
    }
    pub fn resolve(&self, relative: &RelativePath) -> PathBuf {
        self.path.join(relative.as_path())
    }
    pub fn sync(&self) -> Result<(), Error> {
        self.directory.sync_all()?;
        Ok(())
    }
}

pub struct AtomicFile;

impl AtomicFile {
    pub fn create(
        directory: &PrivateDirectory,
        destination: &EntryName,
        bytes: &[u8],
    ) -> Result<(), Error> {
        unix::atomic_create(directory, destination, bytes)
    }

    pub fn is_temporary_name(name: &EntryName) -> bool {
        name.as_os_str()
            .to_str()
            .and_then(|value| value.strip_prefix(".state-atomic-"))
            .and_then(|value| value.strip_suffix(".tmp"))
            .is_some_and(|value| {
                value.len() == 32
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
    }
    pub fn replace(
        directory: &PrivateDirectory,
        destination: &RelativePath,
        bytes: &[u8],
    ) -> Result<(), Error> {
        unix::atomic_replace(directory, destination, bytes)
    }
}

pub struct NoClobberPublish;
impl NoClobberPublish {
    /// Publish a single-linked regular staging file within one held private directory.
    pub fn publish(
        directory: &PrivateDirectory,
        source: &RelativePath,
        destination: &RelativePath,
    ) -> Result<(), Error> {
        unix::no_clobber_publish(directory, source, destination)
    }
}

/// A process-scoped exclusive advisory lock. The lock file remains in place
/// after release so a second process cannot lock a replacement inode while the
/// original holder is still alive.
pub struct AdvisoryLock {
    _file: File,
}

impl AdvisoryLock {
    pub fn acquire(directory: &PrivateDirectory, name: &RelativePath) -> Result<Self, Error> {
        unix::advisory_lock(directory, name)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InventoryLimits {
    pub max_entries: usize,
    pub max_total_bytes: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectoryInventory {
    pub entries: usize,
    pub total_bytes: u64,
}

pub fn bounded_directory_inventory(
    path: &Path,
    limits: InventoryLimits,
) -> Result<DirectoryInventory, Error> {
    unix::bounded_inventory(path, limits)
}

pub fn sync_file_and_parent(path: &Path) -> Result<(), Error> {
    unix::sync_file_and_parent(path)
}
pub fn sync_directory(path: &Path) -> Result<(), Error> {
    unix::open_directory(path)?.sync_all()?;
    Ok(())
}
fn temporary_name() -> Result<String, Error> {
    let mut bytes = [0u8; 16];
    system_entropy::fill(&mut bytes).map_err(Error::Randomness)?;
    let nonce = crate::lower_hex(&bytes);
    Ok(format!(".state-atomic-{nonce}.tmp"))
}

#[cfg(target_os = "linux")]
pub mod linux;

#[derive(Debug, error_definition::Error)]
pub enum Error {
    #[error("{operation}; staging cleanup also failed: {cleanup}")]
    Cleanup {
        #[source]
        operation: Box<Error>,
        cleanup: io::Error,
    },
    #[error("secure randomness unavailable")]
    Randomness(#[source] system_entropy::Error),
    #[error("an absolute path is required: {0:?}")]
    AbsolutePathRequired(PathBuf),
    #[error("unsafe relative path: {0:?}")]
    UnsafeRelativePath(PathBuf),
    #[error("path is not a regular unlinked object: {0:?}")]
    UnsafeFileType(PathBuf),
    #[error("path has multiple links: {0:?}")]
    MultipleLinks(PathBuf),
    #[error("path is not privately owned with exact permissions: {0:?}")]
    UnsafePermissions(PathBuf),
    #[error("directory entry identity changed: {0:?}")]
    IdentityChanged(PathBuf),
    #[error("publication happened but durability could not be confirmed: {0}")]
    PublishedDurabilityUnknown(#[source] io::Error),
    #[error("advisory lock is already held: {0:?}")]
    AlreadyLocked(PathBuf),
    #[error("destination already exists: {0:?}")]
    DestinationExists(PathBuf),
    #[error("directory inventory budget exceeded")]
    BudgetExceeded,
    #[error("{0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
#[path = "../../../../test/platform/unit/fs_safety/tests.rs"]
mod tests;
