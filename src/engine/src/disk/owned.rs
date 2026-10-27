//! Held filesystem resources, separate from serializable configuration and writer locks.
use super::*;
use platform_io::fs_safety::linux::{DirectoryEntries, FileIdentity};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// A path is an address below this held root, never a request to reopen the root.
pub(crate) struct DiskRoot {
    path: PathBuf,
    directory: OpenAt2Root,
}

impl DiskRoot {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let private = existing_private_dir(path)?;
        let path = private.path().to_path_buf();
        let directory = OpenAt2Root::from_private_directory(private, MountPolicy::SameMount)
            .map_err(fs_error)?;
        Ok(Self { path, directory })
    }

    fn relative(&self, path: &Path) -> Result<RelativePath> {
        let relative = path.strip_prefix(&self.path).map_err(|_| {
            Error::new(
                ErrorCode::UnsafePath,
                "address is outside its held disk root",
            )
        })?;
        RelativePath::new(relative).map_err(fs_error)
    }

    fn open_directory(&self, path: &Path) -> Result<File> {
        if path == self.path {
            return Ok(File::from(
                rustix::fs::openat(
                    self.directory.directory(),
                    ".",
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::DIRECTORY
                        | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )
                .map_err(std::io::Error::from)?,
            ));
        }
        self.directory
            .open_directory(&self.relative(path)?)
            .map_err(fs_error)
    }

    pub(crate) fn directory(&self, path: &Path) -> Result<File> {
        let directory = self.open_directory(path)?;
        if path == self.path {
            return Ok(directory);
        }
        let metadata = directory.metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7777 != 0o700
        {
            return Err(fs_error(platform_io::fs_safety::Error::UnsafePermissions(
                path.into(),
            )));
        }
        Ok(directory)
    }

    pub(crate) fn entries(&self, path: &Path) -> Result<DirectoryEntries> {
        if path == self.path {
            self.directory.entries().map_err(fs_error)
        } else {
            self.directory
                .entries_at(&self.relative(path)?)
                .map_err(fs_error)
        }
    }

    pub(crate) fn open_regular(&self, path: &Path) -> Result<File> {
        self.directory
            .open_file(&self.relative(path)?, false)
            .map_err(fs_error)
    }

    /// Absence belongs to the leaf only; open the safe parent before the leaf.
    pub(crate) fn open_optional_regular(&self, path: &Path) -> Result<Option<File>> {
        let (parent, name) = self.parent(path)?;
        match rustix::fs::openat2(
            &parent,
            name.as_os_str(),
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            rustix::fs::ResolveFlags::BENEATH
                | rustix::fs::ResolveFlags::NO_SYMLINKS
                | rustix::fs::ResolveFlags::NO_XDEV,
        ) {
            Ok(fd) => {
                let file = File::from(fd);
                SingleLinkRequirement::check(&file).map_err(fs_error)?;
                Ok(Some(file))
            }
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(error) => Err(fs_error(platform_io::fs_safety::Error::Io(error.into()))),
        }
    }

    pub(crate) fn optional_metadata(&self, path: &Path) -> Result<Option<fs::Metadata>> {
        if path == self.path {
            return Ok(Some(self.directory.directory().metadata()?));
        }
        let (parent, name) = self.parent(path)?;
        match rustix::fs::openat2(
            &parent,
            name.as_os_str(),
            rustix::fs::OFlags::PATH | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
            rustix::fs::ResolveFlags::BENEATH
                | rustix::fs::ResolveFlags::NO_SYMLINKS
                | rustix::fs::ResolveFlags::NO_XDEV,
        ) {
            Ok(fd) => Ok(Some(File::from(fd).metadata()?)),
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(error) => Err(fs_error(platform_io::fs_safety::Error::Io(error.into()))),
        }
    }

    fn parent(&self, path: &Path) -> Result<(File, EntryName)> {
        self.relative(path)?;
        let parent = path
            .parent()
            .expect("invariant: a relative child has a parent");
        let name = EntryName::new(
            path.file_name()
                .expect("invariant: a relative child has a name"),
        )
        .map_err(fs_error)?;
        Ok((self.open_directory(parent)?, name))
    }

    pub(crate) fn read_bytes(&self, path: &Path, maximum: usize) -> Result<Vec<u8>> {
        read_file_bytes(self.open_regular(path)?, maximum)
    }

    pub(crate) fn read_bytes_observed(
        &self,
        path: &Path,
        maximum: usize,
        observe: &mut impl FnMut(usize) -> Result<()>,
    ) -> Result<Vec<u8>> {
        read_file_bytes_observed(self.open_regular(path)?, maximum, observe)
    }

    pub(crate) fn read_json_observed<T: data_serialization::de::DeserializeOwned>(
        &self,
        path: &Path,
        observe: &mut impl FnMut(usize) -> Result<()>,
    ) -> Result<T> {
        decode_strict(&self.read_bytes_observed(path, MAX_METADATA, observe)?).map_err(Into::into)
    }

    pub(crate) fn private_directory(&self, path: &Path) -> Result<PrivateDirectory> {
        PrivateDirectory::from_directory(self.open_directory(path)?, path.to_path_buf())
            .map_err(fs_error)
    }

    pub(crate) fn private_dir(&self, path: &Path) -> Result<()> {
        if path == self.path {
            return Ok(());
        }
        self.relative(path)?;
        let parent_path = path
            .parent()
            .expect("invariant: a relative child has a parent");
        let parent = match self.open_directory(parent_path) {
            Ok(parent) => parent,
            Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                self.private_dir(parent_path)?;
                self.open_directory(parent_path)?
            }
            Err(error) => return Err(error),
        };
        let name = EntryName::new(
            path.file_name()
                .expect("invariant: a relative child has a name"),
        )
        .map_err(fs_error)?;
        let created = match rustix::fs::mkdirat(
            &parent,
            name.as_os_str(),
            rustix::fs::Mode::from_raw_mode(0o700),
        ) {
            Ok(()) => true,
            Err(rustix::io::Errno::EXIST) => false,
            Err(error) => return Err(std::io::Error::from(error).into()),
        };
        let directory = File::from(
            rustix::fs::openat2(
                &parent,
                name.as_os_str(),
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::DIRECTORY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
                rustix::fs::ResolveFlags::BENEATH
                    | rustix::fs::ResolveFlags::NO_SYMLINKS
                    | rustix::fs::ResolveFlags::NO_XDEV,
            )
            .map_err(std::io::Error::from)?,
        );
        let directory =
            PrivateDirectory::from_directory(directory, path.to_path_buf()).map_err(fs_error)?;
        if created {
            directory.sync().map_err(fs_error)?;
            parent.sync_all()?;
        }
        Ok(())
    }

    pub(crate) fn write_atomic(&self, path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
        self.relative(path)?;
        let parent = path
            .parent()
            .expect("invariant: a relative child has a parent");
        self.private_dir(parent)?;
        let directory = self.private_directory(parent)?;
        let name = EntryName::new(
            path.file_name()
                .expect("invariant: a relative child has a name"),
        )
        .map_err(fs_error)?;
        write_atomic_in(&directory, &name, bytes, replace)
    }

    pub(crate) fn write_json<T: Serialize + ?Sized>(
        &self,
        path: &Path,
        value: &T,
        replace: bool,
    ) -> Result<()> {
        self.write_atomic(path, &encode_metadata(value, MAX_METADATA)?, replace)
    }

    pub(crate) fn sync_dir(&self, path: &Path) -> Result<()> {
        self.directory(path)?.sync_all()?;
        Ok(())
    }

    pub(crate) fn remove_file(&self, path: &Path) -> Result<()> {
        let (parent, name) = self.parent(path)?;
        rustix::fs::unlinkat(&parent, name.as_os_str(), rustix::fs::AtFlags::empty())
            .map_err(std::io::Error::from)?;
        Ok(())
    }

    pub(crate) fn available(&self) -> Result<u64> {
        let stat =
            rustix::fs::fstatvfs(self.directory.directory()).map_err(std::io::Error::from)?;
        Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
    }

    pub(crate) fn lock_file(&self) -> Result<File> {
        let file = File::from(
            rustix::fs::openat2(
                self.directory.directory(),
                ".writer.lock",
                rustix::fs::OFlags::CREATE
                    | rustix::fs::OFlags::RDWR
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::NONBLOCK
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::from_raw_mode(0o600),
                rustix::fs::ResolveFlags::BENEATH
                    | rustix::fs::ResolveFlags::NO_SYMLINKS
                    | rustix::fs::ResolveFlags::NO_XDEV,
            )
            .map_err(std::io::Error::from)?,
        );
        Ok(file)
    }
}

pub(crate) struct DiskHandle {
    pub(crate) root: DiskRoot,
    pub(crate) seal: DiskSeal,
}

impl DiskHandle {
    fn open(config: &ValidatedConfig, disk: &DiskConfig, sealed: bool) -> Result<Self> {
        let root = DiskRoot::open(&disk.root)?;
        let seal = identity(config, disk)?;
        if root.directory.identity()
            != (FileIdentity {
                device: seal.device,
                inode: seal.inode,
            })
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "disk root changed during admission",
            ));
        }
        if sealed {
            let stored =
                decode_strict(&root.read_bytes(&disk.root.join("disk.json"), MAX_METADATA)?)?;
            validate_stored_seal(&stored, &seal)?;
        }
        Ok(Self { root, seal })
    }

    pub(crate) fn verify_publication(
        &self,
        directory: &Path,
        version: &FileVersion,
        metadata: &crate::engine::provenance::MetadataReadContext<'_>,
    ) -> Result<()> {
        crate::journal::verify_local_publication_at(
            &self.root,
            directory,
            version,
            &mut |_| Ok(()),
            Some(metadata),
        )
    }

    pub(crate) fn open_payload(
        &self,
        directory: &Path,
        version: &FileVersion,
    ) -> Result<(File, Vec<u8>, ChecksumManifest)> {
        open_payload_at(&self.root, directory, version)
    }

    pub(crate) fn verify_payload(&self, directory: &Path, version: &FileVersion) -> Result<File> {
        let (file, _, manifest) = self.open_payload(directory, version)?;
        verify_open_payload(file, manifest, version, usize::MAX, &mut |_| Ok(()))
    }
}

/// One admission generation. Missing members stay absent until an explicit reopen.
pub(crate) struct DiskResources {
    disks: BTreeMap<String, Arc<DiskHandle>>,
}

impl DiskResources {
    pub(crate) fn open(config: &ValidatedConfig, sealed: bool) -> Result<Self> {
        let mut disks = BTreeMap::new();
        let mut physical = BTreeSet::new();
        for disk in &config.disks {
            if sealed && disk_unavailable(disk)? {
                continue;
            }
            let handle = DiskHandle::open(config, disk, sealed)?;
            if !physical.insert(handle.seal.physical_device.clone()) {
                return Err(Error::new(
                    ErrorCode::DiskNotIndependent,
                    "two slots reference one physical disk",
                ));
            }
            disks.insert(disk.disk_id.clone(), Arc::new(handle));
        }
        Ok(Self { disks })
    }

    pub(crate) fn get(&self, disk: &DiskConfig) -> Result<&Arc<DiskHandle>> {
        self.disks.get(&disk.disk_id).ok_or_else(|| {
            Error::new(
                ErrorCode::DiskUnavailable,
                format!("disk {} was unavailable at admission", disk.slot_id),
            )
        })
    }

    pub(crate) fn handles(&self) -> impl Iterator<Item = &Arc<DiskHandle>> {
        self.disks.values()
    }

    pub(crate) fn available(&self, disk: &DiskConfig) -> Option<&Arc<DiskHandle>> {
        self.disks.get(&disk.disk_id)
    }
}
