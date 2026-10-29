//! Held filesystem resources, separate from serializable configuration and writer locks.
use super::*;
use platform_io::fs_safety::linux::{DirectoryEntries, FileIdentity};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

/// Successful child-and-parent barriers in the immutable metadata-object tree.
/// These names never move in the object protocol. Holding both descriptors
/// prevents inode reuse from making a replaced edge match.
struct DurableDirectory {
    directory: Arc<File>,
    parent: Arc<File>,
}

impl DurableDirectory {
    fn binds(&self, directory: &File, parent: &File) -> Result<bool> {
        Ok(
            FileIdentity::from_fd(self.directory.as_ref())? == FileIdentity::from_fd(directory)?
                && FileIdentity::from_fd(self.parent.as_ref())? == FileIdentity::from_fd(parent)?,
        )
    }
}

const MAX_DIRECTORY_RECEIPTS: usize = 128;

/// A path is an address below this held root, never a request to reopen the root.
pub(crate) struct DiskRoot {
    path: PathBuf,
    directory: OpenAt2Root,
    /// Dropping/reopening a root discards its proofs. Existing object directories
    /// can be failed-creation leftovers, so first object write proves their chain.
    durable_directories: Mutex<BTreeMap<PathBuf, DurableDirectory>>,
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
        Ok(Self {
            path,
            directory,
            durable_directories: Mutex::new(BTreeMap::new()),
        })
    }

    /// Admit a private descendant and retain that exact directory descriptor.
    /// Its diagnostic path remains relative to this held resource, not a fresh
    /// named-path root; enumeration can reopen only "." beneath the admitted FD.
    pub(crate) fn private_child(&self, path: &Path) -> Result<Self> {
        let directory = self.directory(path)?;
        let directory =
            OpenAt2Root::from_directory(directory, MountPolicy::SameMount).map_err(fs_error)?;
        Ok(Self {
            path: path.to_owned(),
            directory,
            durable_directories: Mutex::new(BTreeMap::new()),
        })
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
            self.open_relative_directory(None)
        } else {
            self.open_relative_directory(Some(&self.relative(path)?))
        }
    }

    fn open_relative_directory(&self, path: Option<&RelativePath>) -> Result<File> {
        match path {
            None => Ok(File::from(
                rustix::fs::openat(
                    self.directory.directory(),
                    ".",
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::DIRECTORY
                        | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )
                .map_err(std::io::Error::from)?,
            )),
            Some(path) => self.directory.open_directory(path).map_err(fs_error),
        }
    }

    pub(crate) fn directory(&self, path: &Path) -> Result<File> {
        let directory = self.open_directory(path)?;
        if path == self.path {
            return Ok(directory);
        }
        Self::validate_private_directory(&directory, path)?;
        Ok(directory)
    }

    fn validate_private_directory(directory: &File, path: &Path) -> Result<()> {
        let metadata = directory.metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7777 != 0o700
        {
            return Err(fs_error(platform_io::fs_safety::Error::UnsafePermissions(
                path.into(),
            )));
        }
        Ok(())
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
        let (parent, name) = self.relative(path)?.split_parent();
        Ok((self.open_relative_directory(parent.as_ref())?, name))
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
        self.private_relative_dir(Some(&self.relative(path)?))
    }

    fn private_relative_dir(&self, path: Option<&RelativePath>) -> Result<()> {
        let Some(path) = path else {
            return Ok(());
        };
        let (parent_path, name) = path.split_parent();
        let parent = match self.open_relative_directory(parent_path.as_ref()) {
            Ok(parent) => parent,
            Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                self.private_relative_dir(parent_path.as_ref())?;
                self.open_relative_directory(parent_path.as_ref())?
            }
            Err(error) => return Err(error),
        };
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
        let directory = PrivateDirectory::from_directory(directory, self.path.join(path.as_path()))
            .map_err(fs_error)?;
        if created {
            directory.sync().map_err(fs_error)?;
            parent.sync_all()?;
        }
        Ok(())
    }

    fn durable_directory(&self, path: Option<&RelativePath>) -> Result<Arc<File>> {
        let mut receipts = self
            .durable_directories
            .lock()
            .expect("invariant: directory durability receipt lock poisoned");
        self.ensure_directory(path, &mut receipts)
    }

    fn ensure_directory(
        &self,
        path: Option<&RelativePath>,
        receipts: &mut BTreeMap<PathBuf, DurableDirectory>,
    ) -> Result<Arc<File>> {
        let Some(path) = path else {
            return self.open_relative_directory(None).map(Arc::new);
        };
        let (parent_path, name) = path.split_parent();
        // An existing parent may itself be an interrupted mkdir. Prove every
        // edge from the held root, rather than recursing only on NotFound.
        let parent = self.ensure_directory(parent_path.as_ref(), receipts)?;
        let created = match rustix::fs::mkdirat(
            parent.as_ref(),
            name.as_os_str(),
            rustix::fs::Mode::from_raw_mode(0o700),
        ) {
            Ok(()) => true,
            Err(rustix::io::Errno::EXIST) => false,
            Err(error) => return Err(std::io::Error::from(error).into()),
        };
        let directory = File::from(
            rustix::fs::openat2(
                parent.as_ref(),
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
        Self::validate_private_directory(&directory, &self.path.join(path.as_path()))?;
        if !created
            && let Some(receipt) = receipts.get(path.as_path())
            && receipt.binds(&directory, parent.as_ref())?
        {
            return Ok(Arc::clone(&receipt.directory));
        }
        // Never reuse an old proof after replacement or a failed barrier.
        receipts.remove(path.as_path());
        directory
            .sync_all()
            .map_err(|error| fs_error(platform_io::fs_safety::Error::Io(error)))?;
        parent.sync_all()?;
        let directory = Arc::new(directory);
        // Evict only for an actual new proof, never a cache hit. Eviction drops
        // an optimization: the next use rebuilds the affected ancestor proofs.
        // Each receipt shares handles, not other receipts, so at most twice
        // this many directory descriptors remain retained after the call.
        if receipts.len() == MAX_DIRECTORY_RECEIPTS {
            receipts.clear();
        }
        receipts.insert(
            path.as_path().to_path_buf(),
            DurableDirectory {
                directory: Arc::clone(&directory),
                parent,
            },
        );
        Ok(directory)
    }

    pub(crate) fn write_atomic(&self, path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
        let (parent, name) = self.relative(path)?.split_parent();
        self.private_relative_dir(parent.as_ref())?;
        let directory = PrivateDirectory::from_directory(
            self.open_relative_directory(parent.as_ref())?,
            path.parent()
                .expect("invariant: a relative child has a parent")
                .to_path_buf(),
        )
        .map_err(fs_error)?;
        write_atomic_in(&directory, &name, bytes, replace)
    }

    /// Only the journal's validated address can select this fixed, nonmoving
    /// object tree. Generic container/staging paths keep their own protocols.
    pub(crate) fn write_immutable_object(
        &self,
        address: &crate::journal::ObjectAddress<'_>,
        bytes: &[u8],
        replace: bool,
    ) -> Result<()> {
        let path = address.path(&self.path);
        let (parent, name) = self.relative(&path)?.split_parent();
        let held = self.durable_directory(parent.as_ref())?;
        // Duplicate this admitted descriptor for the writer, never reopen its
        // diagnostic path after completing the edge's durability proof.
        let directory = PrivateDirectory::from_directory(
            held.try_clone()?,
            path.parent()
                .expect("invariant: immutable object has a parent")
                .to_path_buf(),
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
        let seal = identity_at(config, disk, root.directory.directory())?;
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

#[cfg(test)]
#[path = "../../../../test/engine/unit/disk/owned_directory_tests.rs"]
mod directory_tests;
