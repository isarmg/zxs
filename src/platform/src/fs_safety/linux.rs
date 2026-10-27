//! Linux-only openat2 root policy and typed object/lock boundaries.
use crate::fs_safety::{EntryName, Error, PrivateDirectory, RelativePath};
use rustix::{
    fd::AsFd,
    fs::{Dir, FileType, Mode, OFlags, ResolveFlags, fstat, openat, openat2},
};
use std::{
    ffi::OsStr,
    fs::File,
    io,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
}
impl FileIdentity {
    pub fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
    pub fn from_fd(fd: impl AsFd) -> io::Result<Self> {
        let stat = fstat(fd)?;
        Ok(Self {
            device: stat.st_dev,
            inode: stat.st_ino,
        })
    }
}

/// Cross-mount access is an explicit filesystem capability, never a product-name branch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MountPolicy {
    SameMount,
    AllowNestedMounts,
}

#[derive(Debug)]
pub struct OpenAt2Root {
    root: File,
    identity: FileIdentity,
    resolve: ResolveFlags,
}
impl OpenAt2Root {
    pub fn open(path: &Path, mounts: MountPolicy) -> Result<Self, Error> {
        let root = crate::fs_safety::unix::open_directory(path)?;
        Self::from_directory(root, mounts)
    }

    /// Consume an admitted directory handle without reopening its path.
    ///
    /// # Errors
    /// Propagates identity inspection and required openat2 probe failures.
    /// The selected mount policy is unchanged.
    pub fn from_private_directory(
        root: PrivateDirectory,
        mounts: MountPolicy,
    ) -> Result<Self, Error> {
        let identity = FileIdentity::from_fd(&root.directory)?;
        Self::from_admitted_directory(root.directory, identity, mounts)
    }

    pub fn from_directory(root: File, mounts: MountPolicy) -> Result<Self, Error> {
        let stat = fstat(&root).map_err(io::Error::from)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
            return Err(Error::UnsafeFileType("root".into()));
        }
        let identity = FileIdentity {
            device: stat.st_dev,
            inode: stat.st_ino,
        };
        Self::from_admitted_directory(root, identity, mounts)
    }

    fn from_admitted_directory(
        root: File,
        identity: FileIdentity,
        mounts: MountPolicy,
    ) -> Result<Self, Error> {
        let mut resolve = ResolveFlags::BENEATH | ResolveFlags::NO_MAGICLINKS;
        if mounts == MountPolicy::SameMount {
            resolve |= ResolveFlags::NO_XDEV;
        }
        // Probe the required primitive immediately, with no openat/path fallback.
        openat2(
            &root,
            ".",
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            resolve,
        )
        .map_err(io::Error::from)?;
        Ok(Self {
            root,
            identity,
            resolve,
        })
    }
    pub const fn identity(&self) -> FileIdentity {
        self.identity
    }
    pub fn directory(&self) -> &File {
        &self.root
    }
    pub fn into_directory(self) -> File {
        self.root
    }

    /// Open a descendant directory under this root's mount and link policy.
    ///
    /// # Errors
    /// Returns the source error for missing, linked, cross-mount or non-directory paths.
    pub fn open_directory(&self, path: &RelativePath) -> Result<File, Error> {
        Ok(File::from(
            openat2(
                &self.root,
                path.as_path(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
                self.resolve | ResolveFlags::NO_SYMLINKS,
            )
            .map_err(io::Error::from)?,
        ))
    }

    /// Enumerate this held root with a cursor independent of other readers.
    ///
    /// # Errors
    /// Opening or reading the directory preserves its underlying I/O error.
    pub fn entries(&self) -> Result<DirectoryEntries, Error> {
        // Reopen only "." relative to the held FD. A duplicated descriptor
        // would share the directory cursor, and the root may be O_PATH-only.
        let directory = File::from(
            openat(
                &self.root,
                ".",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(io::Error::from)?,
        );
        DirectoryEntries::new(directory)
    }

    /// Enumerate a descendant under the same policy without reconstructing a root.
    ///
    /// # Errors
    /// Propagates directory-open, entry-read and entry-name errors.
    pub fn entries_at(&self, path: &RelativePath) -> Result<DirectoryEntries, Error> {
        DirectoryEntries::new(self.open_directory(path)?)
    }

    /// Open only a single-linked regular file, refusing links in every component.
    /// NONBLOCK prevents a malicious FIFO from hanging before the type check.
    pub fn open_file(&self, path: &RelativePath, write: bool) -> Result<File, Error> {
        let access = if write { OFlags::RDWR } else { OFlags::RDONLY };
        let file = File::from(
            openat2(
                &self.root,
                path.as_path(),
                access | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                Mode::empty(),
                self.resolve | ResolveFlags::NO_SYMLINKS,
            )
            .map_err(io::Error::from)?,
        );
        SingleLinkRequirement::check(&file)?;
        Ok(file)
    }
    pub fn identity_of(&self, path: &RelativePath) -> Result<FileIdentity, Error> {
        Ok(FileIdentity::from_fd(self.open_file(path, false)?)?)
    }
}

/// Streaming directory names; object type and contents remain a later I/O boundary.
pub struct DirectoryEntries {
    directory: Dir,
}

impl DirectoryEntries {
    fn new(directory: File) -> Result<Self, Error> {
        Ok(Self {
            directory: Dir::new(directory).map_err(io::Error::from)?,
        })
    }
}

impl Iterator for DirectoryEntries {
    type Item = Result<EntryName, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let entry = match self.directory.read()? {
                Ok(entry) => entry,
                Err(error) => return Some(Err(io::Error::from(error).into())),
            };
            let name = entry.file_name().to_bytes();
            if matches!(name, b"." | b"..") {
                continue;
            }
            return Some(EntryName::new(OsStr::from_bytes(name)));
        }
    }
}

pub struct SingleLinkRequirement;
impl SingleLinkRequirement {
    pub fn check(fd: impl AsFd) -> Result<FileIdentity, Error> {
        let stat = fstat(fd).map_err(io::Error::from)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(Error::UnsafeFileType("opened object".into()));
        }
        if stat.st_nlink != 1 {
            return Err(Error::MultipleLinks("opened object".into()));
        }
        Ok(FileIdentity {
            device: stat.st_dev,
            inode: stat.st_ino,
        })
    }
}

/// Owns an independent open-file description of the already-anchored directory.
#[derive(Debug)]
pub struct AdvisoryLock {
    directory: File,
}
impl AdvisoryLock {
    pub fn directory(directory: &File) -> Result<Self, Error> {
        if !directory.metadata()?.is_dir() {
            return Err(Error::UnsafeFileType("lock anchor".into()));
        }
        let directory = File::from(
            openat(
                directory,
                ".",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(io::Error::from)?,
        );
        directory.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => Error::AlreadyLocked("directory".into()),
            std::fs::TryLockError::Error(error) => Error::Io(error),
        })?;
        Ok(Self { directory })
    }
}

impl Drop for AdvisoryLock {
    fn drop(&mut self) {
        // A forked child can retain the open-file description until exec, after our FD closes.
        if let Err(error) = self.directory.unlock() {
            tracing::error!(%error, "directory advisory lock release failed");
        }
    }
}

#[cfg(test)]
#[path = "../../../../test/platform/unit/fs_safety/linux/tests.rs"]
mod tests;
