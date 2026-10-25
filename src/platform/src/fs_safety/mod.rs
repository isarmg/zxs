//! Filesystem operations whose names encode their durability and link-safety guarantees.

#[cfg(any(not(unix), test))]
use std::fs;
use std::{
    ffi::OsStr,
    fs::File,
    io,
    path::{Component, Path, PathBuf},
};
#[cfg(not(unix))]
use std::{fs::OpenOptions, io::Write};

#[cfg(unix)]
mod unix;

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
    #[cfg(unix)]
    administration_owner: Option<(u32, u32)>,
}

impl PrivateDirectory {
    /// Create missing private components and reject replacement-capable ancestry.
    /// Root-owned standard ancestors and trusted sticky directories are allowed;
    /// existing directories are validated, never chmodded or re-owned.
    #[cfg(unix)]
    pub fn create_trusted(path: impl AsRef<Path>) -> Result<Self, Error> {
        unix::trusted_private_directory(path.as_ref(), true)
    }
    #[cfg(unix)]
    pub fn open_trusted(path: impl AsRef<Path>) -> Result<Self, Error> {
        unix::trusted_private_directory(path.as_ref(), false)
    }
    /// Open a private single-link payload relative to this held directory.
    #[cfg(unix)]
    pub fn open_payload(&self, name: &EntryName, create: bool) -> Result<File, Error> {
        unix::open_payload(self, name, create)
    }
    /// Detect replacement without ever using the replacement as the payload.
    #[cfg(unix)]
    pub fn verify_payload(&self, name: &EntryName, held: &File) -> Result<(), Error> {
        use std::os::unix::fs::MetadataExt;
        self.verify_identity()?;
        let named = self.open_payload(name, false)?.metadata()?;
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
    #[cfg(unix)]
    pub fn open_for_administration(path: impl AsRef<Path>) -> Result<Self, Error> {
        unix::open_administrative_directory(path.as_ref())
    }
    /// Create or validate a direct child of this held private directory.
    pub fn create_child(&self, name: &EntryName) -> Result<Self, Error> {
        #[cfg(unix)]
        {
            unix::create_private_child(self, name)
        }
        #[cfg(not(unix))]
        {
            Self::create(self.path.join(name.as_path()))
        }
    }

    /// Open an existing private directory without creating or changing anything.
    pub fn open_existing(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        #[cfg(unix)]
        {
            unix::open_private_directory(path)
        }
        #[cfg(not(unix))]
        {
            if !path.is_absolute() {
                return Err(Error::AbsolutePathRequired(path.to_path_buf()));
            }
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(Error::UnsafeFileType(path.to_path_buf()));
            }
            Ok(Self {
                path: path.to_path_buf(),
                directory: File::open(path)?,
            })
        }
    }

    /// Enumerate only regular, single-linked children with hard entry/byte budgets.
    pub fn files(&self, limits: InventoryLimits) -> Result<Vec<FileEntry>, Error> {
        #[cfg(unix)]
        {
            unix::files(self, limits)
        }
        #[cfg(not(unix))]
        {
            portable_files(self, limits)
        }
    }
    pub fn read_bounded(&self, name: &EntryName, max_bytes: usize) -> Result<Vec<u8>, Error> {
        #[cfg(unix)]
        {
            unix::read_bounded(self, name, max_bytes)
        }
        #[cfg(not(unix))]
        {
            use std::io::Read;
            let path = self.path.join(name.as_path());
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(Error::UnsafeFileType(path));
            }
            let mut bytes = Vec::new();
            File::open(&path)?
                .take((max_bytes as u64).saturating_add(1))
                .read_to_end(&mut bytes)?;
            if bytes.len() > max_bytes {
                return Err(Error::BudgetExceeded);
            }
            Ok(bytes)
        }
    }
    pub fn remove_file(&self, name: &EntryName) -> Result<(), Error> {
        #[cfg(unix)]
        {
            unix::remove_file(self, name)
        }
        #[cfg(not(unix))]
        {
            let path = self.path.join(name.as_path());
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(Error::UnsafeFileType(path));
            }
            fs::remove_file(path)?;
            self.sync()
        }
    }
    pub fn create(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        #[cfg(unix)]
        {
            unix::create_private_directory(path)
        }
        #[cfg(not(unix))]
        {
            if !path.is_absolute() {
                return Err(Error::AbsolutePathRequired(path.to_path_buf()));
            }
            match fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => return Err(Error::Io(source)),
            }
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(Error::UnsafeFileType(path.to_path_buf()));
            }
            let directory = File::open(path)?;
            Ok(Self {
                path: path.to_path_buf(),
                directory,
            })
        }
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
        #[cfg(unix)]
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
        #[cfg(not(unix))]
        let _ = held;
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

#[cfg(not(unix))]
fn portable_files(
    directory: &PrivateDirectory,
    limits: InventoryLimits,
) -> Result<Vec<FileEntry>, Error> {
    let mut result = Vec::new();
    let mut total_bytes = 0u64;
    for entry in fs::read_dir(directory.path())? {
        let entry = entry?;
        if result.len() >= limits.max_entries {
            return Err(Error::BudgetExceeded);
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(Error::UnsafeFileType(entry.path()));
        }
        total_bytes = total_bytes
            .checked_add(metadata.len())
            .ok_or(Error::BudgetExceeded)?;
        if total_bytes > limits.max_total_bytes {
            return Err(Error::BudgetExceeded);
        }
        result.push(FileEntry {
            name: EntryName::new(entry.file_name())?,
            bytes: metadata.len(),
        });
    }
    Ok(result)
}
impl AtomicFile {
    pub fn create(
        directory: &PrivateDirectory,
        destination: &EntryName,
        bytes: &[u8],
    ) -> Result<(), Error> {
        #[cfg(unix)]
        {
            unix::atomic_create(directory, destination, bytes)
        }
        #[cfg(not(unix))]
        {
            let temporary = RelativePath::new(temporary_name()?)?;
            Self::replace(directory, &temporary, bytes)?;
            NoClobberPublish::publish(directory, &temporary, &destination.as_relative())
        }
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
        #[cfg(unix)]
        {
            unix::atomic_replace(directory, destination, bytes)
        }
        #[cfg(not(unix))]
        {
            let destination_path = directory.resolve(destination);
            let parent = destination_path
                .parent()
                .ok_or_else(|| Error::UnsafeRelativePath(destination.as_path().to_path_buf()))?;
            if parent != directory.path() {
                fs::create_dir_all(parent)?;
            }
            let temporary = parent.join(temporary_name()?);
            let result = (|| {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temporary)?;
                file.write_all(bytes)?;
                file.sync_all()?;
                fs::rename(&temporary, &destination_path)?;
                sync_directory(parent)?;
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            result
        }
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
        if source.as_path().components().count() != 1
            || destination.as_path().components().count() != 1
        {
            return Err(Error::UnsafeRelativePath(
                destination.as_path().to_path_buf(),
            ));
        }
        #[cfg(unix)]
        {
            unix::no_clobber_publish(directory, source, destination)
        }
        #[cfg(not(unix))]
        {
            let source = directory.resolve(source);
            let destination = directory.resolve(destination);
            if destination.exists() {
                return Err(Error::DestinationExists(destination.to_path_buf()));
            }
            fs::hard_link(&source, &destination).map_err(Error::Io)?;
            let file = File::open(&destination)?;
            file.sync_all()?;
            sync_directory(
                destination
                    .parent()
                    .ok_or_else(|| Error::UnsafeRelativePath(destination.to_path_buf()))?,
            )?;
            fs::remove_file(source)?;
            directory.sync()?;
            Ok(())
        }
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
        #[cfg(unix)]
        {
            unix::advisory_lock(directory, name)
        }
        #[cfg(not(unix))]
        {
            let path = directory.resolve(name);
            if path.parent() != Some(directory.path()) {
                return Err(Error::UnsafeRelativePath(path));
            }
            if let Ok(metadata) = fs::symlink_metadata(&path)
                && metadata.file_type().is_symlink()
            {
                return Err(Error::UnsafeFileType(path));
            }
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)?;
            let metadata = file.metadata()?;
            if !metadata.is_file() {
                return Err(Error::UnsafeFileType(path));
            }
            file.try_lock().map_err(|source| match source {
                std::fs::TryLockError::WouldBlock => Error::AlreadyLocked(path.clone()),
                std::fs::TryLockError::Error(source) => Error::Io(source),
            })?;
            file.sync_all()?;
            directory.sync()?;
            Ok(Self { _file: file })
        }
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
    #[cfg(unix)]
    {
        unix::bounded_inventory(path, limits)
    }
    #[cfg(not(unix))]
    {
        inventory_path(path, limits)
    }
}

#[cfg(not(unix))]
fn inventory_path(path: &Path, limits: InventoryLimits) -> Result<DirectoryInventory, Error> {
    let mut pending = vec![path.to_path_buf()];
    let mut result = DirectoryInventory {
        entries: 0,
        total_bytes: 0,
    };
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() {
                return Err(Error::UnsafeFileType(entry.path()));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.is_file() && metadata.nlink() != 1 {
                    return Err(Error::MultipleLinks(entry.path()));
                }
            }
            result.entries = result.entries.checked_add(1).ok_or(Error::BudgetExceeded)?;
            result.total_bytes = result
                .total_bytes
                .checked_add(metadata.len())
                .ok_or(Error::BudgetExceeded)?;
            if result.entries > limits.max_entries || result.total_bytes > limits.max_total_bytes {
                return Err(Error::BudgetExceeded);
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(result)
}

pub fn sync_file_and_parent(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        unix::sync_file_and_parent(path)
    }
    #[cfg(not(unix))]
    {
        File::open(path)?.sync_all()?;
        sync_directory(
            path.parent()
                .ok_or_else(|| Error::UnsafeRelativePath(path.to_path_buf()))?,
        )
    }
}
pub fn sync_directory(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    unix::open_directory(path)?.sync_all()?;
    #[cfg(not(unix))]
    File::open(path)?.sync_all()?;
    Ok(())
}
fn temporary_name() -> Result<String, Error> {
    let mut bytes = [0u8; 16];
    system_entropy::fill(&mut bytes).map_err(|_| Error::Randomness)?;
    let nonce: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(".state-atomic-{nonce}.tmp"))
}

#[cfg(target_os = "linux")]
pub mod linux;

#[derive(Debug, error_definition::Error)]
pub enum Error {
    #[error("secure randomness unavailable")]
    Randomness,
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
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[cfg(test)]
#[path = "../../../../test/platform/unit/fs_safety/tests.rs"]
mod tests;
