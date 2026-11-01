//! Portable Unix descriptor-relative primitives; intentionally does not need openat2.
use super::{
    AdvisoryLock, EntryName, Error, FileEntry, InventoryLimits, PayloadOpenMode, PrivateDirectory,
    RelativePath, temporary_name,
};
use rustix::{
    fd::AsFd,
    fs::{
        AtFlags, FileType, Mode, OFlags, fstat, mkdirat, open, openat, renameat, statat, unlinkat,
    },
    process::geteuid,
};
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

pub(super) fn open_directory(path: &Path) -> Result<File, Error> {
    if !path.is_absolute() {
        return Err(Error::AbsolutePathRequired(path.to_path_buf()));
    }
    let mut fd = open("/", DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?;
    for component in path.components() {
        match component {
            Component::RootDir => (),
            Component::Normal(name) => {
                fd = openat(&fd, name, DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?;
            }
            _ => return Err(Error::UnsafeRelativePath(path.to_path_buf())),
        }
    }
    Ok(File::from(fd))
}

pub(super) fn open_private_directory(path: &Path) -> Result<PrivateDirectory, Error> {
    let directory = open_directory(path)?;
    validate_private_directory(&directory, path)?;
    Ok(PrivateDirectory {
        path: path.to_path_buf(),
        directory,
        administration_owner: None,
    })
}

pub(super) fn private_directory_from_handle(
    directory: File,
    path: PathBuf,
) -> Result<PrivateDirectory, Error> {
    if !path.is_absolute() {
        return Err(Error::AbsolutePathRequired(path));
    }
    if path
        .components()
        .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(Error::UnsafeRelativePath(path));
    }
    let stat = fstat(&directory).map_err(io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        return Err(Error::UnsafeFileType(path));
    }
    validate_private_permissions(&stat, &path)?;
    Ok(PrivateDirectory {
        path,
        directory,
        administration_owner: None,
    })
}

pub(super) fn open_administrative_directory(path: &Path) -> Result<PrivateDirectory, Error> {
    let directory = open_directory(path)?;
    let stat = fstat(&directory).map_err(io::Error::from)?;
    let caller = geteuid().as_raw();
    if (caller != 0 && caller != stat.st_uid) || stat.st_mode & 0o7777 != 0o700 {
        return Err(Error::UnsafePermissions(path.to_path_buf()));
    }
    Ok(PrivateDirectory {
        path: path.to_path_buf(),
        directory,
        administration_owner: Some((stat.st_uid, stat.st_gid)),
    })
}

fn validate_private_directory(directory: &File, path: &Path) -> Result<(), Error> {
    let stat = fstat(directory).map_err(io::Error::from)?;
    validate_private_permissions(&stat, path)
}

fn validate_private_permissions(stat: &rustix::fs::Stat, path: &Path) -> Result<(), Error> {
    if stat.st_uid != geteuid().as_raw() || stat.st_mode & 0o7777 != 0o700 {
        return Err(Error::UnsafePermissions(path.to_path_buf()));
    }
    Ok(())
}

pub(super) fn create_private_directory(path: &Path) -> Result<PrivateDirectory, Error> {
    if !path.is_absolute() {
        return Err(Error::AbsolutePathRequired(path.to_path_buf()));
    }
    let parent = path
        .parent()
        .ok_or_else(|| Error::UnsafeRelativePath(path.to_path_buf()))?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::UnsafeRelativePath(path.to_path_buf()))?;
    let parent = open_directory(parent)?;
    create_private_at(&parent, name, path, None)
}

/// Recursive creation with the same existing-ancestor admission as `create`.
pub(super) fn create_private_directories(path: &Path) -> Result<PrivateDirectory, Error> {
    if !path.is_absolute() {
        return Err(Error::AbsolutePathRequired(path.to_path_buf()));
    }
    if path.file_name().is_none()
        || path
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(Error::UnsafeRelativePath(path.to_path_buf()));
    }
    let mut directory =
        File::from(open("/", DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?);
    let mut current = PathBuf::from("/");
    for component in path.components() {
        let name = match component {
            Component::RootDir => continue,
            Component::Normal(name) => name,
            _ => return Err(Error::UnsafeRelativePath(path.to_path_buf())),
        };
        current.push(name);
        let next = match openat(&directory, name, DIRECTORY_FLAGS, Mode::empty()) {
            Ok(fd) => {
                let next = File::from(fd);
                if current == path {
                    validate_private_directory(&next, path)?;
                }
                // Existing ancestors can be left by a failed creation attempt.
                next.sync_all()?;
                directory.sync_all()?;
                next
            }
            Err(rustix::io::Errno::NOENT) => {
                // The missing component is itself a private creation target,
                // including when another creator wins the mkdir race.
                create_private_at(&directory, name, &current, None)?.directory
            }
            Err(error) => return Err(io::Error::from(error).into()),
        };
        directory = next;
    }
    Ok(PrivateDirectory {
        path: path.to_path_buf(),
        directory,
        administration_owner: None,
    })
}

pub(super) fn create_private_child(
    parent: &PrivateDirectory,
    name: &EntryName,
) -> Result<PrivateDirectory, Error> {
    create_private_at(
        &parent.directory,
        name.as_os_str(),
        &parent.path.join(name.as_path()),
        parent.administration_owner,
    )
}

fn create_private_at(
    parent: &File,
    name: &OsStr,
    path: &Path,
    administration_owner: Option<(u32, u32)>,
) -> Result<PrivateDirectory, Error> {
    let created = match mkdirat(parent, name, Mode::from_raw_mode(0o700)) {
        Ok(()) => true,
        Err(rustix::io::Errno::EXIST) => false,
        Err(error) => return Err(io::Error::from(error).into()),
    };
    let directory =
        File::from(openat(parent, name, DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?);
    if let Some((uid, gid)) = administration_owner {
        if created {
            // A service-owned parent may be renamed concurrently. Only the
            // newly created inode still owned by this caller may be re-owned.
            validate_private_directory(&directory, path)?;
            rustix::fs::fchown(
                &directory,
                Some(rustix::process::Uid::from_raw(uid)),
                Some(rustix::process::Gid::from_raw(gid)),
            )
            .map_err(io::Error::from)?;
        }
        validate_administration(&directory, path, uid, gid)?;
    } else {
        validate_private_directory(&directory, path)?;
    }
    // Never chmod an existing path: even a rejected symlink must have no effect.
    // EXIST can be a retry after either creation barrier failed.
    directory.sync_all()?;
    parent.sync_all()?;
    Ok(PrivateDirectory {
        path: path.to_path_buf(),
        directory,
        administration_owner,
    })
}

fn entry_name(path: &RelativePath) -> Result<&OsStr, Error> {
    if path.as_path().components().count() != 1 {
        return Err(Error::UnsafeRelativePath(path.as_path().to_path_buf()));
    }
    Ok(path
        .as_path()
        .file_name()
        .expect("invariant: one normal path component has a file name"))
}

pub(super) fn sync_file_and_parent(path: &Path) -> Result<(), Error> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::UnsafeRelativePath(path.to_path_buf()))?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::UnsafeRelativePath(path.to_path_buf()))?;
    let directory = open_directory(parent)?;
    let file = File::from(
        openat(
            &directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?,
    );
    regular_single_link(&file, path)?;
    file.sync_all()?;
    directory.sync_all()?;
    Ok(())
}

pub(super) fn files(
    directory: &PrivateDirectory,
    limits: InventoryLimits,
) -> Result<Vec<FileEntry>, Error> {
    let mut entries = rustix::fs::Dir::read_from(&directory.directory).map_err(io::Error::from)?;
    let mut result = Vec::new();
    let mut total_bytes = 0u64;
    while let Some(entry) = entries.read() {
        let entry = entry.map_err(io::Error::from)?;
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        if result.len() >= limits.max_entries {
            return Err(Error::BudgetExceeded);
        }
        use std::os::unix::ffi::OsStrExt;
        let name = EntryName::new(OsStr::from_bytes(name.to_bytes()))?;
        let file = open_regular(directory, &name)?;
        let bytes = file.metadata()?.len();
        total_bytes = total_bytes
            .checked_add(bytes)
            .ok_or(Error::BudgetExceeded)?;
        if total_bytes > limits.max_total_bytes {
            return Err(Error::BudgetExceeded);
        }
        result.push(FileEntry { name, bytes });
    }
    Ok(result)
}

pub(super) fn open_regular(directory: &PrivateDirectory, name: &EntryName) -> Result<File, Error> {
    verify_administration(directory)?;
    let file = File::from(
        openat(
            &directory.directory,
            name.as_os_str(),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?,
    );
    let identity = validate_opened_regular(directory, &file, name)?;
    let current = statat(
        &directory.directory,
        name.as_os_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(io::Error::from)?;
    if (identity.st_dev, identity.st_ino) != (current.st_dev, current.st_ino) {
        return Err(Error::IdentityChanged(name.as_path().to_path_buf()));
    }
    Ok(file)
}

pub(super) fn validate_opened_regular(
    directory: &PrivateDirectory,
    file: &File,
    name: &EntryName,
) -> Result<rustix::fs::Stat, Error> {
    let identity = regular_single_link(file, name.as_path())?;
    validate_administrative_file(directory, &identity, name.as_path())?;
    Ok(identity)
}

pub(super) fn read_bounded(
    directory: &PrivateDirectory,
    name: &EntryName,
    max_bytes: usize,
) -> Result<Vec<u8>, Error> {
    let file = open_regular(directory, name)?;
    if file.metadata()?.len() > max_bytes as u64 {
        return Err(Error::BudgetExceeded);
    }
    let mut bytes = Vec::new();
    file.take((max_bytes as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(Error::BudgetExceeded);
    }
    Ok(bytes)
}

pub(super) fn remove_file(directory: &PrivateDirectory, name: &EntryName) -> Result<(), Error> {
    let _file = open_regular(directory, name)?;
    unlinkat(&directory.directory, name.as_os_str(), AtFlags::empty()).map_err(io::Error::from)?;
    directory.sync()
}

fn regular_single_link(fd: impl AsFd, path: &Path) -> Result<rustix::fs::Stat, Error> {
    let stat = fstat(fd).map_err(io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(Error::UnsafeFileType(path.to_path_buf()));
    }
    if stat.st_nlink != 1 {
        return Err(Error::MultipleLinks(path.to_path_buf()));
    }
    Ok(stat)
}

enum DestinationState {
    Missing,
    Existing,
}

fn validate_destination(
    directory: &PrivateDirectory,
    name: &OsStr,
) -> Result<DestinationState, Error> {
    verify_administration(directory)?;
    match statat(&directory.directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
                return Err(Error::UnsafeFileType(name.into()));
            }
            if stat.st_nlink != 1 {
                return Err(Error::MultipleLinks(name.into()));
            }
            validate_administrative_file(directory, &stat, Path::new(name))?;
            Ok(DestinationState::Existing)
        }
        Err(rustix::io::Errno::NOENT) => Ok(DestinationState::Missing),
        Err(error) => Err(io::Error::from(error).into()),
    }
}

pub(super) fn atomic_replace(
    directory: &PrivateDirectory,
    destination: &RelativePath,
    bytes: &[u8],
) -> Result<File, Error> {
    atomic_write(directory, destination, bytes, false)
}

pub(super) fn atomic_create(
    directory: &PrivateDirectory,
    destination: &EntryName,
    bytes: &[u8],
) -> Result<(), Error> {
    atomic_write(directory, &destination.as_relative(), bytes, true).map(drop)
}

struct StagingCleanup<'a> {
    directory: &'a File,
    name: String,
    armed: bool,
}
impl StagingCleanup<'_> {
    fn cleanup(&mut self) -> io::Result<()> {
        if !self.armed {
            return Ok(());
        }
        self.armed = false;
        match unlinkat(self.directory, self.name.as_str(), AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
    fn failed(&mut self, operation: Error) -> Error {
        match self.cleanup() {
            Ok(()) => operation,
            Err(cleanup) => Error::Cleanup {
                operation: Box::new(operation),
                cleanup,
            },
        }
    }
}
impl Drop for StagingCleanup<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            tracing::error!(%error, "atomic staging cleanup during unwind or cancellation failed");
        }
    }
}

/// Exclusive, unpublished file owned by one held directory. Dropping cancels it.
/// Only `prepare` can establish the file barrier required for publication.
pub struct StagedAtomicFile<'a> {
    directory: &'a PrivateDirectory,
    destination: EntryName,
    file: File,
    cleanup: StagingCleanup<'a>,
}

/// Complete bytes whose file barrier has succeeded; publication consumes this owner.
pub struct PreparedAtomicFile<'a> {
    staged: StagedAtomicFile<'a>,
    identity: rustix::fs::Stat,
}

/// Evidence about publication: not attempted, attempted with unknown outcome, or published.
#[derive(Debug, error_definition::Error)]
pub enum AtomicCommitError {
    #[error("atomic publication did not occur: {0}")]
    NotPublished(#[source] Error),
    #[error("atomic publication outcome is unknown: {0}")]
    Unknown(#[source] Error),
    #[error("atomic publication occurred but could not be confirmed: {0}")]
    Published(#[source] Error),
}
impl AtomicCommitError {
    /// Discard phase information for callers using the original one-step API.
    pub fn into_source(self) -> Error {
        match self {
            Self::NotPublished(error) | Self::Unknown(error) | Self::Published(error) => error,
        }
    }
}

pub(super) fn atomic_stage<'a>(
    directory: &'a PrivateDirectory,
    destination: &EntryName,
) -> Result<StagedAtomicFile<'a>, Error> {
    verify_administration(directory)?;
    let temporary = temporary_name()?;
    let fd = openat(
        &directory.directory,
        temporary.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(io::Error::from)?;
    Ok(StagedAtomicFile {
        directory,
        destination: destination.clone(),
        file: File::from(fd),
        cleanup: StagingCleanup {
            directory: &directory.directory,
            name: temporary,
            armed: true,
        },
    })
}

impl<'a> StagedAtomicFile<'a> {
    /// Write and synchronize the complete payload without changing the destination.
    /// On failure, cleanup is attempted and its error retains the original source.
    pub fn prepare(mut self, bytes: &[u8]) -> Result<PreparedAtomicFile<'a>, Error> {
        let result = (|| {
            if let Some((uid, gid)) = self.directory.administration_owner {
                rustix::fs::fchown(
                    &self.file,
                    Some(rustix::process::Uid::from_raw(uid)),
                    Some(rustix::process::Gid::from_raw(gid)),
                )
                .map_err(io::Error::from)?;
            }
            let identity = regular_single_link(&self.file, Path::new(&self.cleanup.name))?;
            self.file.write_all(bytes)?;
            self.file.sync_all()?;
            Ok(identity)
        })();
        match result {
            Ok(identity) => Ok(PreparedAtomicFile {
                staged: self,
                identity,
            }),
            Err(error) => Err(self.cleanup.failed(error)),
        }
    }
}

impl PreparedAtomicFile<'_> {
    /// Publish without replacing an existing destination, then sync the held directory.
    /// Success returns the exact published descriptor. Failure reports publication phase.
    pub fn create(self) -> Result<File, AtomicCommitError> {
        self.commit(true)
    }
    /// Replace the destination, then sync the held directory, retaining its descriptor.
    /// This does not prove that a diagnostic directory path still names the held directory;
    /// callers requiring that policy must call `PrivateDirectory::verify_identity`.
    pub fn replace(self) -> Result<File, AtomicCommitError> {
        self.commit(false)
    }
    fn commit(mut self, create_only: bool) -> Result<File, AtomicCommitError> {
        let staged = &mut self.staged;
        let directory = staged.directory;
        let name = staged.destination.as_os_str();
        let checked = (|| {
            if matches!(
                validate_destination(directory, name)?,
                DestinationState::Existing
            ) && create_only
            {
                return Err(Error::DestinationExists(name.into()));
            }
            // A split-phase caller may do other work before commit. Recheck the
            // staging name for both publication modes, never publish a substitute.
            regular_single_link(&staged.file, Path::new(&staged.cleanup.name))?;
            let named = statat(
                &directory.directory,
                staged.cleanup.name.as_str(),
                AtFlags::SYMLINK_NOFOLLOW,
            )
            .map_err(io::Error::from)?;
            if (self.identity.st_dev, self.identity.st_ino) != (named.st_dev, named.st_ino) {
                return Err(Error::IdentityChanged(staged.cleanup.name.clone().into()));
            }
            Ok(())
        })();
        if let Err(error) = checked {
            return Err(AtomicCommitError::NotPublished(
                staged.cleanup.failed(error),
            ));
        }
        // A filesystem can change the namespace yet report an error (e.g. an
        // NFS RPC replay after server failure). Once attempted, neither errno
        // nor a failed reply proves non-publication. Preserve staging evidence.
        staged.cleanup.armed = false;
        let renamed = if create_only {
            rename_no_replace(directory, OsStr::new(&staged.cleanup.name), name)
        } else {
            renameat(
                &directory.directory,
                staged.cleanup.name.as_str(),
                &directory.directory,
                name,
            )
        };
        renamed.map_err(|error| AtomicCommitError::Unknown(Error::Io(error.into())))?;
        let result = (|| {
            let named = statat(&directory.directory, name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|error| Error::PublishedDurabilityUnknown(error.into()))?;
            if (self.identity.st_dev, self.identity.st_ino) != (named.st_dev, named.st_ino) {
                return Err(Error::IdentityChanged(
                    staged.destination.as_path().to_path_buf(),
                ));
            }
            directory
                .directory
                .sync_all()
                .map_err(Error::PublishedDurabilityUnknown)
        })();
        result.map_err(AtomicCommitError::Published)?;
        Ok(self.staged.file)
    }
}

fn atomic_write(
    directory: &PrivateDirectory,
    destination: &RelativePath,
    bytes: &[u8],
    create_only: bool,
) -> Result<File, Error> {
    let name = entry_name(destination)?;
    validate_destination(directory, name)?;
    let prepared = atomic_stage(directory, &EntryName::new(name)?)?.prepare(bytes)?;
    if create_only {
        prepared.create()
    } else {
        prepared.replace()
    }
    .map_err(|error| match error {
        // Keep the current one-step create API's collision category. The
        // staged API (and AuthStore) still receives Unknown with the errno;
        // this projection must never manufacture a NotPublished proof.
        AtomicCommitError::Unknown(Error::Io(source))
            if create_only
                && source.raw_os_error() == Some(rustix::io::Errno::EXIST.raw_os_error()) =>
        {
            Error::DestinationExists(destination.as_path().to_path_buf())
        }
        other => other.into_source(),
    })
}

pub(super) fn advisory_lock(
    directory: &PrivateDirectory,
    name: &RelativePath,
) -> Result<AdvisoryLock, Error> {
    verify_administration(directory)?;
    let name = entry_name(name)?;
    let path = Path::new(name);
    let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let opened = openat(
        &directory.directory,
        name,
        flags | OFlags::CREATE | OFlags::EXCL,
        Mode::from_raw_mode(0o600),
    );
    let (file, created) = match opened {
        Ok(fd) => (File::from(fd), true),
        Err(rustix::io::Errno::EXIST) => (
            File::from(
                openat(&directory.directory, name, flags, Mode::empty())
                    .map_err(io::Error::from)?,
            ),
            false,
        ),
        Err(error) => return Err(io::Error::from(error).into()),
    };
    if created && let Some((uid, gid)) = directory.administration_owner {
        rustix::fs::fchown(
            &file,
            Some(rustix::process::Uid::from_raw(uid)),
            Some(rustix::process::Gid::from_raw(gid)),
        )
        .map_err(io::Error::from)?;
    }
    let identity = regular_single_link(&file, path)?;
    let owner = directory
        .administration_owner
        .map_or(geteuid().as_raw(), |(uid, _)| uid);
    if identity.st_uid != owner || identity.st_mode & 0o7777 != 0o600 {
        return Err(Error::UnsafePermissions(path.to_path_buf()));
    }
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => Error::AlreadyLocked(path.to_path_buf()),
        std::fs::TryLockError::Error(error) => Error::Io(error),
    })?;
    let current =
        statat(&directory.directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
    if (identity.st_dev, identity.st_ino) != (current.st_dev, current.st_ino) {
        return Err(Error::IdentityChanged(path.to_path_buf()));
    }
    file.sync_all()?;
    directory.sync()?;
    Ok(AdvisoryLock { _file: file })
}

fn validate_administration(file: &File, path: &Path, uid: u32, gid: u32) -> Result<(), Error> {
    let stat = fstat(file).map_err(io::Error::from)?;
    if stat.st_uid != uid || stat.st_gid != gid || stat.st_mode & 0o7777 != 0o700 {
        return Err(Error::UnsafePermissions(path.to_path_buf()));
    }
    let caller = geteuid().as_raw();
    if caller != 0 && caller != uid {
        return Err(Error::UnsafePermissions(path.to_path_buf()));
    }
    Ok(())
}

pub(super) fn verify_administration(directory: &PrivateDirectory) -> Result<(), Error> {
    if let Some((uid, gid)) = directory.administration_owner {
        validate_administration(&directory.directory, directory.path(), uid, gid)?;
    }
    Ok(())
}

fn validate_administrative_file(
    directory: &PrivateDirectory,
    stat: &rustix::fs::Stat,
    path: &Path,
) -> Result<(), Error> {
    if let Some((uid, gid)) = directory.administration_owner
        && (stat.st_uid != uid || stat.st_gid != gid || stat.st_mode & 0o7777 != 0o600)
    {
        return Err(Error::UnsafePermissions(path.to_path_buf()));
    }
    Ok(())
}

pub(super) fn no_clobber_publish(
    directory: &PrivateDirectory,
    source: &RelativePath,
    destination: &RelativePath,
    held: &File,
) -> Result<(), AtomicCommitError> {
    let checked = (|| {
        let source = entry_name(source)?;
        let destination = entry_name(destination)?;
        let identity = regular_single_link(held, Path::new(source))?;
        let named = statat(&directory.directory, source, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(io::Error::from)?;
        if (identity.st_dev, identity.st_ino) != (named.st_dev, named.st_ino) {
            return Err(Error::IdentityChanged(source.into()));
        }
        held.sync_all()?;
        Ok((source, destination, identity))
    })();
    let (source, destination, identity) = checked.map_err(AtomicCommitError::NotPublished)?;
    rename_no_replace(directory, source, destination)
        .map_err(|error| AtomicCommitError::Unknown(Error::Io(error.into())))?;
    let confirmed = (|| {
        let current = statat(&directory.directory, destination, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| Error::PublishedDurabilityUnknown(error.into()))?;
        if (identity.st_dev, identity.st_ino) != (current.st_dev, current.st_ino) {
            return Err(Error::IdentityChanged(destination.into()));
        }
        regular_single_link(held, Path::new(destination))?;
        directory
            .directory
            .sync_all()
            .map_err(Error::PublishedDurabilityUnknown)
    })();
    confirmed.map_err(AtomicCommitError::Published)
}

// Both callers sync their held source file before this namespace-only step and
// verify the published inode before completing the destination directory barrier.
fn rename_no_replace(
    directory: &PrivateDirectory,
    source: &OsStr,
    destination: &OsStr,
) -> rustix::io::Result<()> {
    rustix::fs::renameat_with(
        &directory.directory,
        source,
        &directory.directory,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )
}

/// DFS keeps at most 128 directory handles, independent of directory width.
pub(super) fn bounded_inventory(
    path: &Path,
    limits: crate::fs_safety::InventoryLimits,
) -> Result<crate::fs_safety::DirectoryInventory, Error> {
    use rustix::fs::Dir;
    let root = open_directory(path)?;
    let mut stack = vec![Dir::new(root).map_err(io::Error::from)?];
    let mut result = crate::fs_safety::DirectoryInventory {
        entries: 0,
        total_bytes: 0,
    };
    while let Some(current) = stack.last_mut() {
        let Some(entry) = current.read() else {
            stack.pop();
            continue;
        };
        let entry = entry.map_err(io::Error::from)?;
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        result.entries = result.entries.checked_add(1).ok_or(Error::BudgetExceeded)?;
        if result.entries > limits.max_entries {
            return Err(Error::BudgetExceeded);
        }
        let parent = current.fd().map_err(io::Error::from)?;
        let stat = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
        let kind = FileType::from_raw_mode(stat.st_mode);
        if !matches!(kind, FileType::Directory | FileType::RegularFile) {
            return Err(Error::UnsafeFileType(
                name.to_string_lossy().into_owned().into(),
            ));
        }
        let fd = openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        let opened = fstat(&fd).map_err(io::Error::from)?;
        if (opened.st_dev, opened.st_ino, opened.st_mode)
            != (stat.st_dev, stat.st_ino, stat.st_mode)
        {
            return Err(Error::IdentityChanged(
                name.to_string_lossy().into_owned().into(),
            ));
        }
        if kind == FileType::RegularFile && opened.st_nlink != 1 {
            return Err(Error::MultipleLinks(
                name.to_string_lossy().into_owned().into(),
            ));
        }
        result.total_bytes = result
            .total_bytes
            .checked_add(u64::try_from(opened.st_size).map_err(|_| Error::BudgetExceeded)?)
            .ok_or(Error::BudgetExceeded)?;
        if result.total_bytes > limits.max_total_bytes {
            return Err(Error::BudgetExceeded);
        }
        if kind == FileType::Directory {
            if stack.len() >= 128 {
                return Err(Error::BudgetExceeded);
            }
            stack.push(Dir::new(fd).map_err(io::Error::from)?);
        }
    }
    Ok(result)
}

// A writable sticky directory protects entries owned by the service, but a
// foreign owner can chmod/rename its own subtree even when it is currently 0700.
fn trusted_ancestor(owner: u32, mode: u32, caller: u32) -> bool {
    (owner == 0 || owner == caller) && (mode & 0o022 == 0 || mode & 0o1000 != 0)
}

pub(super) fn trusted_private_directory(
    path: &Path,
    create: bool,
) -> Result<PrivateDirectory, Error> {
    if !path.is_absolute() {
        return Err(Error::AbsolutePathRequired(path.to_path_buf()));
    }
    let mut directory =
        File::from(open("/", DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?);
    let mut current = std::path::PathBuf::from("/");
    for component in path.components() {
        let name = match component {
            Component::RootDir => continue,
            Component::Normal(name) => name,
            _ => return Err(Error::UnsafeRelativePath(path.to_path_buf())),
        };
        let parent = fstat(&directory).map_err(io::Error::from)?;
        if !trusted_ancestor(parent.st_uid, parent.st_mode, geteuid().as_raw()) {
            return Err(Error::UnsafePermissions(current));
        }
        current.push(name);
        let mut created = false;
        let next = match openat(&directory, name, DIRECTORY_FLAGS, Mode::empty()) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) if create => {
                match mkdirat(&directory, name, Mode::from_raw_mode(0o700)) {
                    Ok(()) => created = true,
                    Err(rustix::io::Errno::EXIST) => (),
                    Err(error) => return Err(io::Error::from(error).into()),
                }
                openat(&directory, name, DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?
            }
            Err(error) => return Err(io::Error::from(error).into()),
        };
        let next = File::from(next);
        let stat = fstat(&next).map_err(io::Error::from)?;
        if !trusted_ancestor(stat.st_uid, stat.st_mode, geteuid().as_raw()) {
            return Err(Error::UnsafePermissions(current));
        }
        if created {
            validate_private_directory(&next, &current)?;
        }
        if create {
            // Reopening an ancestor does not prove its name survived a prior
            // failed creation attempt. Repair every traversed name barrier.
            next.sync_all()?;
            directory.sync_all()?;
        }
        directory = next;
    }
    validate_private_directory(&directory, path)?;
    Ok(PrivateDirectory {
        path: path.to_path_buf(),
        directory,
        administration_owner: None,
    })
}

pub(super) fn open_payload(
    directory: &PrivateDirectory,
    name: &EntryName,
    mode: PayloadOpenMode,
) -> Result<(File, rustix::fs::Stat), Error> {
    let flags = OFlags::RDWR
        | OFlags::CLOEXEC
        | OFlags::NOFOLLOW
        | OFlags::NONBLOCK
        | match mode {
            PayloadOpenMode::CreateNew => OFlags::CREATE | OFlags::EXCL,
            PayloadOpenMode::Existing => OFlags::empty(),
        };
    let file = File::from(
        openat(
            &directory.directory,
            name.as_os_str(),
            flags,
            Mode::from_raw_mode(0o600),
        )
        .map_err(io::Error::from)?,
    );
    let stat = fstat(&file).map_err(io::Error::from)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || stat.st_nlink != 1 {
        return Err(Error::UnsafeFileType(directory.path.join(name.as_path())));
    }
    if stat.st_uid != geteuid().as_raw() || stat.st_mode & 0o7777 != 0o600 {
        return Err(Error::UnsafePermissions(
            directory.path.join(name.as_path()),
        ));
    }
    Ok((file, stat))
}

#[cfg(test)]
#[path = "../../../../test/platform/unit/fs_safety/unix/tests.rs"]
mod tests;
