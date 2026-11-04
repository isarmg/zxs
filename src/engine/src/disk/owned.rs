//! Held filesystem resources, separate from serializable configuration and writer locks.
use super::admission::DiskIoResource;
use super::*;
use platform_io::fs_safety::linux::DirectoryEntries;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// A path is an address below this held root, never a request to reopen the root.
pub(crate) struct DiskRoot {
    path: PathBuf,
    directory: OpenAt2Root,
    io: Option<DiskIoBinding>,
}

/// A directory cursor never exposes its underlying stream or descriptor. Bound
/// cursors retain the opening operation and are revocable while the caller is
/// idle. Unbound bootstrap cursors remain explicitly separate from disk access.
pub(crate) struct DiskDirectoryEntries {
    source: Option<DirectorySource>,
}

enum DirectorySource {
    Bound(DiskIoResource<DirectoryEntries>),
    Unbound(DirectoryEntries),
}

impl DiskDirectoryEntries {
    fn new(entries: DirectoryEntries, lease: Option<DiskIoLease>) -> Result<Self> {
        let source = match lease {
            Some(lease) => {
                let entries = lease.hold(entries);
                entries.check_running()?;
                DirectorySource::Bound(entries)
            }
            None => DirectorySource::Unbound(entries),
        };
        Ok(Self {
            source: Some(source),
        })
    }
}

impl Iterator for DiskDirectoryEntries {
    type Item = std::result::Result<EntryName, platform_io::fs_safety::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let next = match self.source.as_mut()? {
            DirectorySource::Bound(entries) => entries.next_entry(),
            DirectorySource::Unbound(entries) => entries.next(),
        };
        // An error is not an empty/successful inventory. Report it exactly once,
        // then close the stream; EOF also retires ownership while self survives.
        if !matches!(&next, Some(Ok(_))) {
            self.source = None;
        }
        next
    }
}

impl std::iter::FusedIterator for DiskDirectoryEntries {}

impl DiskRoot {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    /// Unbound admission/offline/system root. This does not infer disk authority
    /// from a path. Runtime disk consumers must retain an already bound root;
    /// named-path bootstrap callers remain a separate migration boundary.
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let private = existing_private_dir(path)?;
        let path = private.path().to_path_buf();
        let directory = OpenAt2Root::from_private_directory(private, MountPolicy::SameMount)
            .map_err(fs_error)?;
        Ok(Self {
            path,
            directory,
            io: None,
        })
    }

    /// Bootstrap an explicitly held, not-yet-bound directory. Never use this to
    /// turn a bound root back into an unbound one; use retained_root instead.
    pub(crate) fn from_directory(path: PathBuf, directory: File) -> Result<Self> {
        let private =
            PrivateDirectory::from_directory(directory, path.clone()).map_err(fs_error)?;
        let directory = OpenAt2Root::from_private_directory(private, MountPolicy::SameMount)
            .map_err(fs_error)?;
        // Validate every named ancestor without using that reopened resource as
        // the destination. All writes retain the caller's original descriptor.
        let named = Self::open(&path)?;
        if directory.identity() != named.directory.identity() {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "selected storage directory changed before initialization",
            ));
        }
        Ok(Self {
            path,
            directory,
            io: None,
        })
    }

    fn bind_io(&mut self, seal: &DiskSeal, config: &ValidatedConfig) -> Result<()> {
        self.io = Some(match &self.io {
            Some(binding) => binding.for_authority(seal, config)?,
            None => DiskIoBinding::new(seal, config)?,
        });
        Ok(())
    }

    pub(crate) fn io_binding(&self) -> Result<&DiskIoBinding> {
        self.io.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::ConfigInvalid,
                "root has no admitted disk I/O binding",
            )
        })
    }

    pub(crate) fn admit_io(&self) -> Result<DiskIoLease> {
        self.io_binding()?.admit()
    }

    /// Opening and the real file lifetime share the same admitted operation.
    /// Legacy raw-file entry points still need migration before offline is safe.
    pub(crate) fn open_leased_regular(&self, path: &Path) -> Result<DiskFile> {
        let operation = RootOperation {
            root: self,
            lease: Some(self.admit_io()?),
        };
        let file = operation.open_regular(path)?;
        let file = operation
            .lease
            .expect("invariant: leased open owns an admitted operation")
            .hold(file);
        file.check_running()?;
        Ok(file)
    }

    /// Preserve this root's already established provenance. Bootstrap roots are
    /// explicitly unbound; a managed root never falls back after admission fails.
    pub(crate) fn open_read_regular(&self, path: &Path) -> Result<ReadFile> {
        match &self.io {
            Some(_) => self.open_leased_regular(path).map(Into::into),
            None => self.open_regular(path).map(Into::into),
        }
    }

    /// Admit a private descendant and retain that exact directory descriptor.
    /// Its diagnostic path remains relative to this held resource, not a fresh
    /// named-path root; enumeration can reopen only "." beneath the admitted FD.
    pub(crate) fn private_child(&self, path: &Path) -> Result<Self> {
        let operation = self.operation()?;
        let directory = operation.directory(path)?;
        let directory =
            OpenAt2Root::from_directory(directory, MountPolicy::SameMount).map_err(fs_error)?;
        Ok(Self {
            path: path.to_owned(),
            directory,
            io: self.io.clone(),
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

    /// One synchronous operation, including every nested syscall and cleanup.
    /// A bound root never falls back to bootstrap access on admission failure.
    fn operation(&self) -> Result<RootOperation<'_>> {
        let lease = match &self.io {
            Some(binding) => Some(binding.admit()?),
            None => None,
        };
        Ok(RootOperation { root: self, lease })
    }

    /// Transfer the opening operation into the actual directory stream. Late
    /// registration after quiesce closes the stream instead of escaping the gate.
    pub(crate) fn entries(&self, path: &Path) -> Result<DiskDirectoryEntries> {
        let operation = self.operation()?;
        let entries = operation.entries(path)?;
        DiskDirectoryEntries::new(entries, operation.lease)
    }

    /// Return identity evidence only, with the opened directory closed before
    /// its operation retires. No raw descriptor escapes the admission lifetime.
    pub(crate) fn directory_identity(
        &self,
        path: &Path,
    ) -> Result<platform_io::fs_safety::linux::FileIdentity> {
        let operation = self.operation()?;
        let directory = operation.directory(path)?;
        Ok(platform_io::fs_safety::linux::FileIdentity::from_fd(
            &directory,
        )?)
    }

    // These legacy resource-returning methods admit their opening and validation
    // only. The returned raw resource is NOT tracked after return. Their callers
    // must migrate before disk-wide drain can be used for a production offline.
    pub(crate) fn directory(&self, path: &Path) -> Result<File> {
        self.operation()?.directory(path)
    }

    pub(crate) fn open_regular(&self, path: &Path) -> Result<File> {
        self.operation()?.open_regular(path)
    }

    pub(crate) fn open_optional_regular(&self, path: &Path) -> Result<Option<File>> {
        self.operation()?.open_optional_regular(path)
    }

    pub(crate) fn private_directory(&self, path: &Path) -> Result<PrivateDirectory> {
        self.operation()?.private_directory(path)
    }

    /// The writer lock remains a separate fencing owner, never a revocable reader.
    pub(crate) fn lock_file(&self) -> Result<File> {
        self.operation()?.lock_file()
    }

    pub(crate) fn optional_metadata(&self, path: &Path) -> Result<Option<fs::Metadata>> {
        self.operation()?.optional_metadata(path)
    }

    pub(crate) fn read_bytes(&self, path: &Path, maximum: usize) -> Result<Vec<u8>> {
        self.operation()?.read_bytes(path, maximum)
    }

    pub(crate) fn read_bytes_observed(
        &self,
        path: &Path,
        maximum: usize,
        observe: &mut impl FnMut(usize) -> Result<()>,
    ) -> Result<Vec<u8>> {
        self.operation()?
            .read_bytes_observed(path, maximum, observe)
    }

    pub(crate) fn read_json_observed<T: data_serialization::de::DeserializeOwned>(
        &self,
        path: &Path,
        observe: &mut impl FnMut(usize) -> Result<()>,
    ) -> Result<T> {
        self.operation()?.read_json_observed(path, observe)
    }

    pub(crate) fn private_dir(&self, path: &Path) -> Result<()> {
        self.operation()?.private_dir(path)
    }

    pub(crate) fn write_atomic(&self, path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
        self.operation()?.write_atomic(path, bytes, replace)
    }

    pub(crate) fn write_json<T: Serialize + ?Sized>(
        &self,
        path: &Path,
        value: &T,
        replace: bool,
    ) -> Result<()> {
        self.operation()?.write_json(path, value, replace)
    }

    pub(crate) fn sync_dir(&self, path: &Path) -> Result<()> {
        self.operation()?.sync_dir(path)
    }

    pub(crate) fn remove_file(&self, path: &Path) -> Result<()> {
        self.operation()?.remove_file(path)
    }

    pub(crate) fn available(&self) -> Result<u64> {
        self.operation()?.available()
    }

    /// Spend the transition's unique control lease on a scoped metadata action.
    /// The capability cannot escape this call, clone itself, open raw resources,
    /// derive roots, or admit another operation. There is no production or
    /// external fault API for metadata transition writes yet.
    #[cfg(test)]
    pub(crate) fn with_transition_io<T>(
        &self,
        transition: &mut super::admission::DiskIoTransition,
        run: impl FnOnce(&RootControl<'_>) -> Result<T>,
    ) -> Result<T> {
        let lease = transition.take_drained_control(self.io_binding()?)?;
        let control = RootControl {
            operation: RootOperation {
                root: self,
                lease: Some(lease),
            },
        };
        run(&control)
    }
}

/// Only this module can construct a syscall capability. Nested helpers borrow
/// this same operation rather than attempting admission again. Once admitted,
/// a synchronous operation may finish after quiesce, including its error cleanup;
/// its lease retires only after its stack-owned files and atomic writers do.
struct RootOperation<'a> {
    root: &'a DiskRoot,
    lease: Option<DiskIoLease>,
}

impl RootOperation<'_> {
    /// Keep the exact revocation domain when a successor retains this binding.
    /// This never creates a new active gate for an existing disk.
    fn retained_root(&self, path: PathBuf) -> Result<DiskRoot> {
        let private =
            PrivateDirectory::from_directory(self.directory(self.root.path())?, path.clone())
                .map_err(fs_error)?;
        let directory = OpenAt2Root::from_private_directory(private, MountPolicy::SameMount)
            .map_err(fs_error)?;
        // Revalidate the named address under the original lease, never by
        // constructing an unbound DiskRoot that could become an escape hatch.
        let named = OpenAt2Root::from_private_directory(
            existing_private_dir(&path)?,
            MountPolicy::SameMount,
        )
        .map_err(fs_error)?;
        if directory.identity() != named.identity() {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "selected storage directory changed before generation retention",
            ));
        }
        Ok(DiskRoot {
            path,
            directory,
            io: self.root.io.clone(),
        })
    }

    fn open_directory(&self, path: &Path) -> Result<File> {
        if path == self.root.path {
            self.open_relative_directory(None)
        } else {
            self.open_relative_directory(Some(&self.root.relative(path)?))
        }
    }

    fn open_relative_directory(&self, path: Option<&RelativePath>) -> Result<File> {
        match path {
            None => Ok(File::from(
                rustix::fs::openat(
                    self.root.directory.directory(),
                    ".",
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::DIRECTORY
                        | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )
                .map_err(std::io::Error::from)?,
            )),
            Some(path) => self.root.directory.open_directory(path).map_err(fs_error),
        }
    }

    fn directory(&self, path: &Path) -> Result<File> {
        let directory = self.open_directory(path)?;
        if path == self.root.path {
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

    fn entries(&self, path: &Path) -> Result<DirectoryEntries> {
        if path == self.root.path {
            self.root.directory.entries().map_err(fs_error)
        } else {
            self.root
                .directory
                .entries_at(&self.root.relative(path)?)
                .map_err(fs_error)
        }
    }

    fn open_regular(&self, path: &Path) -> Result<File> {
        self.root
            .directory
            .open_file(&self.root.relative(path)?, false)
            .map_err(fs_error)
    }

    /// Absence belongs to the leaf only; open the safe parent before the leaf.
    fn open_optional_regular(&self, path: &Path) -> Result<Option<File>> {
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

    fn optional_metadata(&self, path: &Path) -> Result<Option<fs::Metadata>> {
        if path == self.root.path {
            return Ok(Some(self.root.directory.directory().metadata()?));
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
        let (parent, name) = self.root.relative(path)?.split_parent();
        Ok((self.open_relative_directory(parent.as_ref())?, name))
    }

    fn read_bytes(&self, path: &Path, maximum: usize) -> Result<Vec<u8>> {
        read_file_bytes(self.open_regular(path)?, maximum)
    }

    fn read_bytes_observed(
        &self,
        path: &Path,
        maximum: usize,
        observe: &mut impl FnMut(usize) -> Result<()>,
    ) -> Result<Vec<u8>> {
        read_file_bytes_observed(self.open_regular(path)?, maximum, observe)
    }

    fn read_json_observed<T: data_serialization::de::DeserializeOwned>(
        &self,
        path: &Path,
        observe: &mut impl FnMut(usize) -> Result<()>,
    ) -> Result<T> {
        decode_strict(&self.read_bytes_observed(path, MAX_METADATA, observe)?).map_err(Into::into)
    }

    fn private_directory(&self, path: &Path) -> Result<PrivateDirectory> {
        PrivateDirectory::from_directory(self.open_directory(path)?, path.to_path_buf())
            .map_err(fs_error)
    }

    fn private_dir(&self, path: &Path) -> Result<()> {
        if path == self.root.path {
            return Ok(());
        }
        self.private_relative_dir(Some(&self.root.relative(path)?))
    }

    fn private_relative_dir(&self, path: Option<&RelativePath>) -> Result<()> {
        let Some(path) = path else {
            return Ok(());
        };
        let (parent_path, name) = path.split_parent();
        // A previous attempt may have created any ancestor before its fsync
        // failed. Admit and synchronize the complete chain, not just the leaf.
        self.private_relative_dir(parent_path.as_ref())?;
        let parent = self.open_relative_directory(parent_path.as_ref())?;
        match rustix::fs::mkdirat(
            &parent,
            name.as_os_str(),
            rustix::fs::Mode::from_raw_mode(0o700),
        ) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => (),
            Err(error) => return Err(std::io::Error::from(error).into()),
        }
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
            PrivateDirectory::from_directory(directory, self.root.path.join(path.as_path()))
                .map_err(fs_error)?;
        directory.sync().map_err(fs_error)?;
        parent.sync_all()?;
        Ok(())
    }

    fn write_atomic(&self, path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
        let (parent, name) = self.root.relative(path)?.split_parent();
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

    fn write_json<T: Serialize + ?Sized>(
        &self,
        path: &Path,
        value: &T,
        replace: bool,
    ) -> Result<()> {
        self.write_atomic(path, &encode_metadata(value, MAX_METADATA)?, replace)
    }

    fn sync_dir(&self, path: &Path) -> Result<()> {
        self.directory(path)?.sync_all()?;
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        let (parent, name) = self.parent(path)?;
        rustix::fs::unlinkat(&parent, name.as_os_str(), rustix::fs::AtFlags::empty())
            .map_err(std::io::Error::from)?;
        Ok(())
    }

    fn available(&self) -> Result<u64> {
        let stat =
            rustix::fs::fstatvfs(self.root.directory.directory()).map_err(std::io::Error::from)?;
        Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
    }

    fn lock_file(&self) -> Result<File> {
        let file = File::from(
            rustix::fs::openat2(
                self.root.directory.directory(),
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

/// Transition-only metadata capability. Its underlying operation is private and
/// borrowed by the callback, so neither its root nor its unique lease can escape.
#[cfg(test)]
pub(crate) struct RootControl<'a> {
    operation: RootOperation<'a>,
}

#[cfg(test)]
impl RootControl<'_> {
    pub(crate) fn read_metadata(&self, path: &Path) -> Result<Vec<u8>> {
        self.operation.read_bytes(path, MAX_METADATA)
    }

    pub(crate) fn write_json<T: Serialize + ?Sized>(
        &self,
        path: &Path,
        value: &T,
        replace: bool,
    ) -> Result<()> {
        self.operation.write_json(path, value, replace)
    }
}

pub(crate) struct DiskHandle {
    pub(crate) root: DiskRoot,
    pub(crate) seal: DiskSeal,
}

impl DiskHandle {
    pub(crate) fn new(
        mut root: DiskRoot,
        seal: DiskSeal,
        config: &ValidatedConfig,
    ) -> Result<Self> {
        root.bind_io(&seal, config)?;
        Ok(Self { root, seal })
    }

    pub(crate) fn open(config: &ValidatedConfig, disk: &DiskConfig, sealed: bool) -> Result<Self> {
        let root = DiskRoot::open(&disk.root)?;
        let seal = identity_at(config, disk, root.directory.directory())?;
        if sealed {
            let bytes = root.read_bytes(&disk.root.join("disk.json"), MAX_METADATA)?;
            validate_stored_seal(&decode_seal(&bytes)?, &seal)?;
        }
        Self::new(root, seal, config)
    }

    pub(crate) fn open_payload(
        &self,
        directory: &Path,
        version: &FileVersion,
    ) -> Result<(ReadFile, Vec<u8>, ChecksumManifest)> {
        open_payload_at(&self.root, directory, version)
    }

    pub(crate) fn verify_payload(
        &self,
        directory: &Path,
        version: &FileVersion,
    ) -> Result<ReadFile> {
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
        for disk in &config.disks {
            if sealed && disk_unavailable(disk)? {
                continue;
            }
            let handle = DiskHandle::open(config, disk, sealed)?;
            disks.insert(disk.disk_id.clone(), Arc::new(handle));
        }
        Ok(Self { disks })
    }

    pub(crate) fn open_activation(config: &ValidatedConfig) -> Result<Self> {
        let mut disks = BTreeMap::new();
        for disk in &config.disks {
            let root = DiskRoot::open(&disk.root)?;
            let seal = identity_at(config, disk, &root.directory(root.path())?)?;
            let bytes = root.read_bytes(&disk.root.join("disk.json"), MAX_METADATA)?;
            crate::engine::membership_transition::pending::activation::predecessor_seal_matches(
                config, disk, &root, &bytes, &seal,
            )?;
            disks.insert(
                disk.disk_id.clone(),
                Arc::new(DiskHandle::new(root, seal, config)?),
            );
        }
        Ok(Self { disks })
    }
    pub(crate) fn initialize_held(
        config: &ValidatedConfig,
        directories: Vec<File>,
    ) -> Result<Self> {
        if directories.len() != config.disks.len() {
            return Err(Error::new(
                ErrorCode::ConfigInvalid,
                "held storage directory count differs from configuration",
            ));
        }
        let mut disks = BTreeMap::new();
        let mut identities = BTreeSet::new();
        for (disk, directory) in config.disks.iter().zip(directories) {
            let root = DiskRoot::from_directory(disk.root.clone(), directory)?;
            let identity = root.directory.identity();
            if !identities.insert((identity.device, identity.inode)) {
                return Err(Error::new(
                    ErrorCode::ConfigInvalid,
                    "selected directories name the same held storage root",
                ));
            }
            if root.entries(&disk.root)?.next().transpose()?.is_some() {
                return Err(Error::new(
                    ErrorCode::DiskAlreadyEnrolled,
                    "storage activation requires an empty held directory; existing contents are preserved",
                ));
            }
            let seal = identity_at(config, disk, root.directory.directory())?;
            disks.insert(
                disk.disk_id.clone(),
                Arc::new(DiskHandle::new(root, seal, config)?),
            );
        }
        Ok(Self { disks })
    }

    pub(crate) fn recover_held(config: &ValidatedConfig, directories: Vec<File>) -> Result<Self> {
        if directories.len() != config.disks.len() {
            return Err(Error::new(
                ErrorCode::ConfigInvalid,
                "held recovery directory count differs from configuration",
            ));
        }
        let mut disks = BTreeMap::new();
        let mut identities = BTreeSet::new();
        for (disk, directory) in config.disks.iter().zip(directories) {
            let root = DiskRoot::from_directory(disk.root.clone(), directory)?;
            let identity = root.directory.identity();
            if !identities.insert((identity.device, identity.inode)) {
                return Err(Error::new(
                    ErrorCode::ConfigInvalid,
                    "selected recovery directories name the same held storage root",
                ));
            }
            let seal = identity_at(config, disk, root.directory.directory())?;
            let bytes = root.read_bytes(&disk.root.join("disk.json"), MAX_METADATA)?;
            validate_stored_seal(&decode_seal(&bytes)?, &seal)?;
            disks.insert(
                disk.disk_id.clone(),
                Arc::new(DiskHandle::new(root, seal, config)?),
            );
        }
        Ok(Self { disks })
    }

    pub(crate) fn generation_handles(
        config: &ValidatedConfig,
        handles: Vec<Arc<DiskHandle>>,
    ) -> Result<Self> {
        if config.generation().is_none() {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "retained resources require an admitted membership generation",
            ));
        }
        let mut disks = BTreeMap::new();
        let mut bindings = Vec::new();
        for handle in handles {
            let operation = handle.root.operation()?;
            let disk = config
                .disks
                .iter()
                .find(|disk| disk.disk_id == handle.seal.disk_id)
                .expect("invariant: retained generation disk");
            let root = operation.retained_root(disk.root.clone())?;
            let seal = identity_at(config, disk, &operation.directory(handle.root.path())?)?;
            bindings.push(handle.root.io_binding()?.clone());
            disks.insert(
                disk.disk_id.clone(),
                Arc::new(DiskHandle::new(root, seal, config)?),
            );
        }
        DiskIoBinding::activate_generation(&bindings, config)?;
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

    /// Remove rejected recovery resources from this admission without dropping
    /// the Engine's separately held writer locks or changing configured voters.
    pub(crate) fn excluding(&self, rejected: &BTreeSet<String>) -> Self {
        Self {
            disks: self
                .disks
                .iter()
                .filter(|(id, _)| !rejected.contains(*id))
                .map(|(id, handle)| (id.clone(), Arc::clone(handle)))
                .collect(),
        }
    }

    pub(crate) fn handles(&self) -> impl Iterator<Item = &Arc<DiskHandle>> {
        self.disks.values()
    }

    pub(crate) fn available(&self, disk: &DiskConfig) -> Option<&Arc<DiskHandle>> {
        self.disks.get(&disk.disk_id)
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/disk/directory_retry_tests.rs"]
mod directory_retry_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/disk/root_operation_tests.rs"]
mod root_operation_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/disk/directory_entries_tests.rs"]
mod directory_entries_tests;
