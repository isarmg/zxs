//! Portable Unix descriptor-relative primitives; intentionally does not need openat2.
use super::{
    AdvisoryLock, EntryName, Error, FileEntry, InventoryLimits, PayloadOpenMode, PrivateDirectory,
    RelativePath, temporary_name,
};
use rustix::{
    fd::{AsFd, OwnedFd},
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
    if created {
        directory.sync_all()?;
        parent.sync_all()?;
    }
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

pub(super) fn entry_metadata(
    directory: &PrivateDirectory,
    name: &EntryName,
) -> Result<super::EntryMetadata, Error> {
    let metadata = statat(
        &directory.directory,
        name.as_os_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(io::Error::from)?;
    match FileType::from_raw_mode(metadata.st_mode) {
        FileType::Directory => Ok(super::EntryMetadata::Directory),
        FileType::RegularFile if metadata.st_nlink == 1 => Ok(super::EntryMetadata::File {
            bytes: u64::try_from(metadata.st_size)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        }),
        FileType::RegularFile => Err(Error::MultipleLinks(name.as_path().to_owned())),
        _ => Err(Error::UnsafeFileType(name.as_path().to_owned())),
    }
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

pub(super) fn remove_files(directory: &PrivateDirectory, names: &[EntryName]) -> Result<(), Error> {
    directory.verify_identity()?;
    for name in names {
        let _file = match open_regular(directory, name) {
            Ok(file) => file,
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        match unlinkat(&directory.directory, name.as_os_str(), AtFlags::empty()) {
            Ok(()) => {}
            Err(error) if error == rustix::io::Errno::NOENT => {}
            Err(error) => return Err(io::Error::from(error).into()),
        }
    }
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

fn validate_destination(directory: &PrivateDirectory, name: &OsStr) -> Result<(), Error> {
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
            Ok(())
        }
        Err(rustix::io::Errno::NOENT) => Ok(()),
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
    name: &'a str,
    armed: bool,
}
impl StagingCleanup<'_> {
    fn cleanup(&mut self) -> io::Result<()> {
        if !self.armed {
            return Ok(());
        }
        self.armed = false;
        match unlinkat(self.directory, self.name, AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}
impl Drop for StagingCleanup<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            tracing::error!(%error, "atomic staging cleanup during unwind failed");
        }
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
    let (temporary, mut file) = create_staging(directory)?;
    let mut cleanup = StagingCleanup {
        directory: &directory.directory,
        name: &temporary,
        armed: true,
    };
    let result = (|| {
        let identity = write_staging(directory, &mut file, &temporary, bytes)?;
        file.sync_all()?;
        validate_destination(directory, name)?;
        if create_only {
            publish_staging(directory, &temporary, name, &file, &identity)?;
        } else {
            renameat(
                &directory.directory,
                temporary.as_str(),
                &directory.directory,
                name,
            )
            .map_err(io::Error::from)?;
        }
        cleanup.armed = false;
        verify_published(directory, name, &identity)?;
        directory
            .directory
            .sync_all()
            .map_err(Error::PublishedDurabilityUnknown)
    })();
    finish_staging(result, &mut cleanup)?;
    Ok(file)
}

fn create_staging(directory: &PrivateDirectory) -> Result<(String, File), Error> {
    let temporary = temporary_name()?;
    let fd: OwnedFd = openat(
        &directory.directory,
        temporary.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(io::Error::from)?;
    Ok((temporary, File::from(fd)))
}

pub(super) fn create_recovery_intent(
    directory: &PrivateDirectory,
    destination: &EntryName,
    bytes: &[u8],
) -> Result<(), Error> {
    let name = destination.as_os_str();
    validate_destination(directory, name)?;
    let (temporary, mut file) = create_staging(directory)?;
    let mut cleanup = StagingCleanup {
        directory: &directory.directory,
        name: &temporary,
        armed: true,
    };
    let result = (|| {
        let identity = write_staging(directory, &mut file, &temporary, bytes)?;
        validate_destination(directory, name)?;
        publish_staging(directory, &temporary, name, &file, &identity)?;
        cleanup.armed = false;
        verify_published(directory, name, &identity)?;
        file.sync_all().map_err(Error::PublishedDurabilityUnknown)?;
        directory
            .directory
            .sync_all()
            .map_err(Error::PublishedDurabilityUnknown)
    })();
    finish_staging(result, &mut cleanup)
}

fn write_staging(
    directory: &PrivateDirectory,
    file: &mut File,
    temporary: &str,
    bytes: &[u8],
) -> Result<rustix::fs::Stat, Error> {
    if let Some((uid, gid)) = directory.administration_owner {
        // Only the exclusive new temporary descriptor is re-owned. The
        // destination inode is never chowned, chmodded, or followed.
        rustix::fs::fchown(
            &*file,
            Some(rustix::process::Uid::from_raw(uid)),
            Some(rustix::process::Gid::from_raw(gid)),
        )
        .map_err(io::Error::from)?;
    }
    let identity = regular_single_link(&*file, Path::new(temporary))?;
    file.write_all(bytes)?;
    Ok(identity)
}

fn publish_staging(
    directory: &PrivateDirectory,
    temporary: &str,
    name: &OsStr,
    file: &File,
    identity: &rustix::fs::Stat,
) -> Result<(), Error> {
    regular_single_link(file, Path::new(temporary))?;
    let staged = statat(&directory.directory, temporary, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(io::Error::from)?;
    if (identity.st_dev, identity.st_ino) != (staged.st_dev, staged.st_ino) {
        return Err(Error::IdentityChanged(temporary.into()));
    }
    rename_no_replace(directory, OsStr::new(temporary), name)
}

fn verify_published(
    directory: &PrivateDirectory,
    name: &OsStr,
    identity: &rustix::fs::Stat,
) -> Result<(), Error> {
    let published = statat(&directory.directory, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| Error::PublishedDurabilityUnknown(error.into()))?;
    if (identity.st_dev, identity.st_ino) != (published.st_dev, published.st_ino) {
        return Err(Error::IdentityChanged(name.into()));
    }
    Ok(())
}

fn finish_staging(
    result: Result<(), Error>,
    cleanup: &mut StagingCleanup<'_>,
) -> Result<(), Error> {
    match result {
        Ok(()) => Ok(()),
        Err(operation) => match cleanup.cleanup() {
            Ok(()) => Err(operation),
            Err(cleanup) => Err(Error::Cleanup {
                operation: Box::new(operation),
                cleanup,
            }),
        },
    }
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
) -> Result<(), Error> {
    let source = entry_name(source)?;
    let destination = entry_name(destination)?;
    let file = File::from(
        openat(
            &directory.directory,
            source,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?,
    );
    let identity = regular_single_link(&file, Path::new(source))?;
    file.sync_all()?;
    rename_no_replace(directory, source, destination)?;
    let current = statat(&directory.directory, destination, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| Error::PublishedDurabilityUnknown(error.into()))?;
    if (identity.st_dev, identity.st_ino) != (current.st_dev, current.st_ino) {
        return Err(Error::IdentityChanged(destination.into()));
    }
    directory
        .directory
        .sync_all()
        .map_err(Error::PublishedDurabilityUnknown)
}

// Both callers sync their held source file before this namespace-only step and
// verify the published inode before completing the destination directory barrier.
fn rename_no_replace(
    directory: &PrivateDirectory,
    source: &OsStr,
    destination: &OsStr,
) -> Result<(), Error> {
    rustix::fs::renameat_with(
        &directory.directory,
        source,
        &directory.directory,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(|error| {
        if error == rustix::io::Errno::EXIST {
            Error::DestinationExists(destination.into())
        } else {
            io::Error::from(error).into()
        }
    })
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
