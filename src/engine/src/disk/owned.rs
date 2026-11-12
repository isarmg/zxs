//! Held filesystem resources, separate from serializable configuration and writer locks.
use super::admission::DiskIoResource;
use super::admission::FinalResource;
use super::*;
use crate::persist::layout;
use platform_io::fs_safety::linux::DirectoryEntries;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, OnceLock, RwLock, RwLockReadGuard},
};

mod install;
pub(crate) use install::{FileStamp, HeldRegular, InstallExchange};

#[path = "working_directory.rs"]
mod working_directory;
pub(crate) use working_directory::{
    DirectoryCreateOutcome, DirectoryInstallOutcome, DirectoryIoError, RepositoryLeaf,
    WorkingDirectory, WorkingDirectoryIdentity,
};

/// A path is an address below this held root, never a request to reopen the root.
pub(crate) struct DiskRoot {
    path: PathBuf,
    directory: Arc<HeldRoot>,
    io: Option<DiskIoBinding>,
}

/// One descriptor shared by generation views. The initial binding is permanent:
/// an older unbound alias cannot turn an adopted or closed root into bootstrap I/O.
#[derive(Debug)]
struct HeldRoot {
    directory: Arc<RootDescriptor>,
    binding: OnceLock<DiskIoBinding>,
}

impl HeldRoot {
    fn access(&self) -> Result<RwLockReadGuard<'_, Option<OpenAt2Root>>> {
        self.directory.access()
    }
}

/// This gate-owned cell never owns a gate, avoiding both a strong cycle and the
/// weak-upgrade race with the last root wrapper's descriptor teardown.
#[derive(Debug)]
struct RootDescriptor {
    directory: RwLock<Option<OpenAt2Root>>,
}

impl RootDescriptor {
    fn access(&self) -> Result<RwLockReadGuard<'_, Option<OpenAt2Root>>> {
        let directory = self
            .directory
            .read()
            .map_err(|_| Error::new(ErrorCode::RecoveryRequired, "held disk root lock poisoned"))?;
        if directory.is_none() {
            return Err(Error::new(
                ErrorCode::DiskUnavailable,
                "held disk root is closed",
            ));
        }
        Ok(directory)
    }
}

impl FinalResource for RootDescriptor {
    fn key(&self) -> usize {
        std::ptr::from_ref(self) as usize
    }

    fn close(&self) {
        let mut directory = match self.directory.write() {
            Ok(directory) => directory,
            Err(poisoned) => poisoned.into_inner(),
        };
        let retired = directory.take();
        // A second closer must wait for the actual close, not only observe None.
        // OpenAt2Root has no lease/destructor callback into this resource or gate.
        drop(retired);
        drop(directory);
    }
}

impl Drop for HeldRoot {
    fn drop(&mut self) {
        // A concurrent finalizer owns the same descriptor cell until this close
        // has finished, even after this wrapper's last Arc is gone.
        self.directory.close();
        if let Some(binding) = self.binding.get() {
            binding.unregister_root(self.directory.key());
        }
    }
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
    fn held(path: PathBuf, directory: OpenAt2Root, io: Option<DiskIoBinding>) -> Result<Self> {
        let root = Self {
            path,
            directory: Arc::new(HeldRoot {
                directory: Arc::new(RootDescriptor {
                    directory: RwLock::new(Some(directory)),
                }),
                binding: OnceLock::new(),
            }),
            io,
        };
        if let Some(binding) = &root.io {
            root.register(binding)?;
        }
        Ok(root)
    }

    fn register(&self, binding: &DiskIoBinding) -> Result<()> {
        let _admission = binding.admit()?;
        if let Some(existing) = self.directory.binding.get() {
            return Self::same_domain(existing, binding);
        }
        let directory =
            self.directory.directory.directory.write().map_err(|_| {
                Error::new(ErrorCode::RecoveryRequired, "held disk root lock poisoned")
            })?;
        if directory.is_none() {
            return Err(Error::new(
                ErrorCode::DiskUnavailable,
                "held disk root is closed",
            ));
        }
        let existing = self.directory.binding.get_or_init(|| binding.clone());
        Self::same_domain(existing, binding)?;
        let erased: Arc<dyn FinalResource> = self.directory.directory.clone();
        let result = binding.register_root(&erased);
        drop(directory);
        result
    }

    fn same_domain(existing: &DiskIoBinding, requested: &DiskIoBinding) -> Result<()> {
        if existing.same_domain(requested) {
            Ok(())
        } else {
            Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "held root already belongs to another disk I/O binding",
            ))
        }
    }

    fn binding(&self) -> Option<&DiskIoBinding> {
        self.io.as_ref().or_else(|| self.directory.binding.get())
    }

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
        Self::held(path, directory, None)
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
        if directory.identity() != named.operation()?.held().identity() {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "selected storage directory changed before initialization",
            ));
        }
        Self::held(path, directory, None)
    }

    fn bind_io(&mut self, seal: &DiskSeal, config: &ValidatedConfig) -> Result<()> {
        let binding = match self.binding() {
            Some(binding) => binding.for_authority(seal, config)?,
            None => DiskIoBinding::new(seal, config)?,
        };
        self.register(&binding)?;
        self.io = Some(binding);
        Ok(())
    }

    pub(crate) fn io_binding(&self) -> Result<&DiskIoBinding> {
        self.binding().ok_or_else(|| {
            Error::new(
                ErrorCode::ConfigInvalid,
                "root has no admitted disk I/O binding",
            )
        })
    }

    pub(crate) fn admit_io(&self) -> Result<DiskIoLease> {
        self.io_binding()?.admit()
    }

    /// Explicit bootstrap provenance, never a fallback after failed admission.
    /// The shared binding also fences aliases created before the root was bound.
    fn optional_io_lease(&self) -> Result<Option<DiskIoLease>> {
        self.binding().map(DiskIoBinding::admit).transpose()
    }

    /// A synchronous multi-step protocol retains this admission until every
    /// stack-owned descriptor, atomic staging file and cleanup tail is dropped.
    /// None means an explicitly unbound bootstrap root, never failed admission.
    /// This does not grant transition-control access or permit a raw FD escape.
    pub(crate) fn retain_io_activity(&self) -> Result<Option<DiskIoLease>> {
        self.optional_io_lease()
    }

    /// Transfer this exact opening responsibility into the journal's private
    /// composite owner. Neither its descriptor nor its lease escapes separately.
    pub(crate) fn journal_directory(&self, path: &Path) -> Result<JournalIo> {
        let operation = self.operation()?;
        let parent = operation.directory(path)?;
        let RootOperation {
            directory, lease, ..
        } = operation;
        drop(directory);
        JournalIo::new(parent, lease)
    }

    /// Opening and the real file lifetime share the same admitted operation.
    /// Legacy raw-file entry points still need migration before offline is safe.
    pub(crate) fn open_leased_regular(&self, path: &Path) -> Result<DiskFile> {
        let operation = self.operation_with(Some(self.admit_io()?))?;
        let file = operation.open_regular(path)?;
        let RootOperation {
            directory, lease, ..
        } = operation;
        drop(directory);
        let file = lease
            .expect("invariant: leased open owns an admitted operation")
            .hold(file);
        file.check_running()?;
        Ok(file)
    }

    /// Preserve this root's already established provenance. Bootstrap roots are
    /// explicitly unbound; a managed root never falls back after admission fails.
    pub(crate) fn open_read_regular(&self, path: &Path) -> Result<ReadFile> {
        let operation = self.operation()?;
        let file = operation.open_regular(path)?;
        operation.into_read_file(file)
    }

    pub(crate) fn open_optional_read_regular(&self, path: &Path) -> Result<Option<ReadFile>> {
        let operation = self.operation()?;
        let file = operation.open_optional_regular(path)?;
        file.map(|file| operation.into_read_file(file)).transpose()
    }

    pub(crate) fn open_optional_read_relative(
        &self,
        path: &RelativePath,
    ) -> Result<Option<ReadFile>> {
        let operation = self.operation()?;
        let file = operation.open_optional_relative_regular(path)?;
        file.map(|file| operation.into_read_file(file)).transpose()
    }

    /// Copy, error cleanup and publication share one operation. Only the synced
    /// file survives, with that same admission transferred into a revocable owner.
    pub(crate) fn verified_copy<'a>(
        &self,
        source: impl std::os::unix::fs::FileExt,
        destination: &Path,
        version: &'a FileVersion,
        max_bytes: u64,
    ) -> std::result::Result<VerifiedCopy<'a>, CopyFailure> {
        let operation = self.operation()?;
        let parent = destination
            .parent()
            .expect("invariant: payload destination has a parent");
        operation.private_dir(parent)?;
        let directory = operation.directory(parent)?;
        let name = EntryName::new(
            destination
                .file_name()
                .expect("invariant: payload destination has a name"),
        )
        .map_err(fs_error)?;
        let (_, _, checksums, file) = full_copy_checked_observed(
            &source,
            &directory,
            &name,
            version.checksum_chunk_bytes,
            max_bytes,
            Some(version),
            &mut |progress| match progress {
                FullCopyProgress::Check => operation.check_running(),
                // Once the name changes, its directory barrier still belongs
                // to this operation even if cancellation has just arrived.
                FullCopyProgress::Created { .. } | FullCopyProgress::Published => Ok(()),
            },
        )?;
        drop(directory);
        let file = operation.into_read_file(file)?;
        VerifiedCopy::new(file, version, checksums)
    }

    /// Retain the destination operation through frozen-byte comparison, staging
    /// cleanup, rename, file/directory synchronization and final descriptor close.
    /// No raw descriptor or publication receipt escapes this operation.
    pub(crate) fn copy_verified_bytes_synced(
        &self,
        path: &Path,
        input: &mut impl Read,
        expectation: platform_io::payload::Expectation,
    ) -> Result<()> {
        let operation = self.operation()?;
        let directory = operation.private_directory(path.parent().ok_or_else(|| {
            Error::new(ErrorCode::UnsafePath, "copy destination has no parent")
        })?)?;
        match operation.open_optional_regular(path)? {
            Some(mut file) => {
                match platform_io::payload::verify_full_observed(
                    &mut file,
                    expectation,
                    &mut || {
                        operation.check_running().map_err(|error| {
                            std::io::Error::new(std::io::ErrorKind::PermissionDenied, error)
                        })
                    },
                ) {
                    Ok(_) => (),
                    Err(
                        error @ (platform_io::payload::Error::Length { .. }
                        | platform_io::payload::Error::Digest),
                    ) => {
                        return Err(Error::from(error)
                            .context(ErrorCode::ObjectCollision, "existing frozen bytes differ"));
                    }
                    Err(error) => return Err(error.into()),
                }
                file.sync_all()?;
                directory.sync().map_err(fs_error)?;
            }
            None => {
                let name =
                    platform_io::fs_safety::EntryName::new(path.file_name().ok_or_else(|| {
                        Error::new(ErrorCode::UnsafePath, "copy destination has no name")
                    })?)
                    .map_err(fs_error)?;
                let mut reader = OperationCopyReader {
                    input,
                    operation: &operation,
                };
                platform_io::payload::write_full(&directory, &name, &mut reader, expectation)?;
            }
        }
        // Once publication starts, finish durability and cleanup even when
        // quiescing arrives. Report cancellation after that tail, never success.
        operation.check_running()
    }

    /// Admit a private descendant and retain that exact directory descriptor.
    /// Its diagnostic path remains relative to this held resource, not a fresh
    /// named-path root; enumeration can reopen only "." beneath the admitted FD.
    pub(crate) fn private_child(&self, path: &Path) -> Result<Self> {
        let operation = self.operation()?;
        let directory = operation.directory(path)?;
        let directory =
            OpenAt2Root::from_directory(directory, MountPolicy::SameMount).map_err(fs_error)?;
        Self::held(path.to_owned(), directory, self.binding().cloned())
    }

    /// Check the intended entity on the very descriptor that all later child
    /// operations retain. A later replacement of its name cannot retarget I/O.
    pub(crate) fn private_child_matching(
        &self,
        path: &Path,
        expected: platform_io::fs_safety::linux::FileIdentity,
    ) -> Result<Self> {
        let operation = self.operation()?;
        let directory = operation.directory(path)?;
        if platform_io::fs_safety::linux::FileIdentity::from_fd(&directory)? != expected {
            return Err(Error::new(
                ErrorCode::ContainerLayoutInvalid,
                "database entity identity changed before retaining its directory",
            ));
        }
        let directory =
            OpenAt2Root::from_directory(directory, MountPolicy::SameMount).map_err(fs_error)?;
        Self::held(path.to_owned(), directory, self.binding().cloned())
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
        let directory = self.directory.access()?;
        let lease = self.optional_io_lease()?;
        Ok(RootOperation {
            root: self,
            directory,
            lease,
        })
    }

    fn operation_with(&self, lease: Option<DiskIoLease>) -> Result<RootOperation<'_>> {
        Ok(RootOperation {
            root: self,
            directory: self.directory.access()?,
            lease,
        })
    }

    pub(crate) fn retained(&self, path: PathBuf) -> Result<Self> {
        self.operation()?.retained_root(path)
    }

    /// Transfer the opening operation into the actual directory stream. Late
    /// registration after quiesce closes the stream instead of escaping the gate.
    pub(crate) fn entries(&self, path: &Path) -> Result<DiskDirectoryEntries> {
        let operation = self.operation()?;
        let entries = operation.entries(path)?;
        let RootOperation {
            directory, lease, ..
        } = operation;
        drop(directory);
        DiskDirectoryEntries::new(entries, lease)
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

    pub(crate) fn open_regular_entry(&self, name: &EntryName) -> Result<File> {
        self.operation()?.open_relative_regular(&name.as_relative())
    }

    pub(crate) fn open_read_entry(&self, name: &EntryName) -> Result<ReadFile> {
        let operation = self.operation()?;
        let file = operation.open_relative_regular(&name.as_relative())?;
        operation.into_read_file(file)
    }

    pub(crate) fn open_optional_read_entry(&self, name: &EntryName) -> Result<Option<ReadFile>> {
        let operation = self.operation()?;
        let file = operation.open_optional_relative_regular(&name.as_relative())?;
        file.map(|file| operation.into_read_file(file)).transpose()
    }

    pub(crate) fn read_entry_bytes(&self, name: &EntryName, maximum: usize) -> Result<Vec<u8>> {
        let operation = self.operation()?;
        read_file_bytes_observed(
            operation.open_relative_regular(&name.as_relative())?,
            maximum,
            &mut |_| operation.check_running(),
        )
    }

    pub(crate) fn write_entry_json<T: Serialize + ?Sized>(
        &self,
        name: &EntryName,
        value: &T,
        replace: bool,
    ) -> Result<()> {
        self.operation()?.write_relative_atomic(
            &name.as_relative(),
            &encode_metadata(value, MAX_METADATA)?,
            replace,
        )
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

    /// Create exactly one new private directory. An existing empty name is not
    /// adopted. The new directory and its parent are synchronized while the
    /// admitting operation still owns both descriptors.
    pub(crate) fn create_private_directory_synced(
        &self,
        path: &Path,
    ) -> Result<platform_io::fs_safety::linux::FileIdentity> {
        self.operation()?.create_private_directory_synced(path)
    }

    pub(crate) fn write_atomic(&self, path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
        self.operation()?.write_atomic(path, bytes, replace)
    }

    pub(crate) fn write_relative_atomic(
        &self,
        path: &RelativePath,
        bytes: &[u8],
        replace: bool,
    ) -> Result<()> {
        self.operation()?.write_relative_atomic(path, bytes, replace)
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

    /// Both directory descriptors and all durability tails stay inside one
    /// admitted operation. No pathname mutation can outlive its writer fence.
    pub(crate) fn rename_noreplace_synced(&self, from: &Path, to: &Path) -> Result<()> {
        self.operation()?
            .rename_noreplace_synced(from, to, RenameSyncOrder::SourceFirst)
    }

    /// Synchronize the destination before the source-name removal. Publication
    /// and proof quarantine retain this order if cancellation follows rename.
    pub(crate) fn rename_noreplace_destination_first_synced(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<()> {
        self.operation()?
            .rename_noreplace_synced(from, to, RenameSyncOrder::DestinationFirst)
    }

    pub(crate) fn remove_file_synced(&self, path: &Path) -> Result<()> {
        self.operation()?
            .remove_synced(path, rustix::fs::AtFlags::empty())
    }

    /// Retry a published cleanup without treating failed durability as absence.
    /// An absent leaf still needs its parent synced after an uncertain unlink.
    pub(crate) fn remove_file_if_present_synced(&self, path: &Path) -> Result<bool> {
        self.operation()?.remove_file_if_present_synced(path)
    }

    pub(crate) fn remove_empty_directory_synced(&self, path: &Path) -> Result<()> {
        self.operation()?
            .remove_synced(path, rustix::fs::AtFlags::REMOVEDIR)
    }

    /// Remove only this writer's unreferenced empty preparation. Admission,
    /// inode verification, the held parent and the sync tail share one lease.
    pub(crate) fn remove_owned_empty_directory_synced(
        &self,
        path: &Path,
        expected: platform_io::fs_safety::linux::FileIdentity,
    ) -> Result<()> {
        self.operation()?
            .remove_owned_empty_directory_synced(path, expected)
    }

    pub(crate) fn component_name_maximum(&self) -> Result<u64> {
        self.operation()?.component_name_maximum()
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
            operation: self.operation_with(Some(lease))?,
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
    // Release the descriptor borrow before the operation can trigger final close.
    directory: RwLockReadGuard<'a, Option<OpenAt2Root>>,
    lease: Option<DiskIoLease>,
}

struct OperationCopyReader<'a, 'root, R> {
    input: &'a mut R,
    operation: &'a RootOperation<'root>,
}
impl<R: Read> Read for OperationCopyReader<'_, '_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.operation
            .check_running()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::PermissionDenied, error))?;
        let maximum = bytes.len().min(64 * 1024);
        let read = self.input.read(&mut bytes[..maximum])?;
        self.operation
            .check_running()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::PermissionDenied, error))?;
        Ok(read)
    }
}

enum RenameSyncOrder {
    SourceFirst,
    DestinationFirst,
}

impl RootOperation<'_> {
    fn into_read_file(self, file: File) -> Result<ReadFile> {
        let Self {
            directory, lease, ..
        } = self;
        // Late registration can finalize synchronously. Close the root borrow
        // before transferring this operation, never its actual file lifetime.
        drop(directory);
        let file = match lease {
            Some(lease) => ReadFile::from(lease.hold(file)),
            None => ReadFile::from(file),
        };
        file.check_running()?;
        Ok(file)
    }

    fn check_running(&self) -> Result<()> {
        match &self.lease {
            Some(lease) => lease.check_running(),
            None => Ok(()),
        }
    }
    fn held(&self) -> &OpenAt2Root {
        self.directory
            .as_ref()
            .expect("invariant: root operation owns an open directory")
    }
    /// Keep the exact revocation domain when a successor retains this binding.
    /// This never creates a new active gate for an existing disk.
    fn retained_root(&self, path: PathBuf) -> Result<DiskRoot> {
        // Revalidate the named address under the original lease, never by
        // constructing an unbound DiskRoot that could become an escape hatch.
        let named = OpenAt2Root::from_private_directory(
            existing_private_dir(&path)?,
            MountPolicy::SameMount,
        )
        .map_err(fs_error)?;
        if self.held().identity() != named.identity() {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "selected storage directory changed before generation retention",
            ));
        }
        Ok(DiskRoot {
            path,
            directory: Arc::clone(&self.root.directory),
            io: self.root.binding().cloned(),
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
                    self.held().directory(),
                    ".",
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::DIRECTORY
                        | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )
                .map_err(std::io::Error::from)?,
            )),
            Some(path) => self.held().open_directory(path).map_err(fs_error),
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
            self.held().entries().map_err(fs_error)
        } else {
            self.held()
                .entries_at(&self.root.relative(path)?)
                .map_err(fs_error)
        }
    }

    fn open_regular(&self, path: &Path) -> Result<File> {
        self.open_relative_regular(&self.root.relative(path)?)
    }

    fn open_relative_regular(&self, path: &RelativePath) -> Result<File> {
        self.held().open_file(path, false).map_err(fs_error)
    }

    /// Absence belongs to the leaf only; open the safe parent before the leaf.
    fn open_optional_regular(&self, path: &Path) -> Result<Option<File>> {
        self.open_optional_relative_regular(&self.root.relative(path)?)
    }

    fn open_optional_relative_regular(&self, path: &RelativePath) -> Result<Option<File>> {
        let (parent, name) = path.split_parent();
        let parent = self.open_relative_directory(parent.as_ref())?;
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
            return Ok(Some(self.held().directory().metadata()?));
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

    fn private_parent(&self, path: &Path) -> Result<(File, EntryName)> {
        let (_, name) = self.root.relative(path)?.split_parent();
        let parent = path
            .parent()
            .expect("invariant: relative child has a parent");
        Ok((self.directory(parent)?, name))
    }

    fn read_bytes(&self, path: &Path, maximum: usize) -> Result<Vec<u8>> {
        read_file_bytes_observed(self.open_regular(path)?, maximum, &mut |_| {
            self.check_running()
        })
    }

    fn read_bytes_observed(
        &self,
        path: &Path,
        maximum: usize,
        observe: &mut impl FnMut(usize) -> Result<()>,
    ) -> Result<Vec<u8>> {
        read_file_bytes_observed(self.open_regular(path)?, maximum, &mut |bytes| {
            observe(bytes)?;
            self.check_running()
        })
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

    fn create_private_directory_synced(
        &self,
        path: &Path,
    ) -> Result<platform_io::fs_safety::linux::FileIdentity> {
        let (parent, name) = self.private_parent(path)?;
        self.check_running()?;
        rustix::fs::mkdirat(
            &parent,
            name.as_os_str(),
            rustix::fs::Mode::from_raw_mode(0o700),
        )
        .map_err(std::io::Error::from)?;
        // The mkdir is already visible. Finish its durability tail under this
        // same lease even if quiesce/cancellation begins now; no descriptor is
        // returned or allowed to escape the operation.
        let directory = File::from(
            rustix::fs::openat2(
                &parent,
                name.as_os_str(),
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::DIRECTORY
                    | rustix::fs::OFlags::CLOEXEC
                    | rustix::fs::OFlags::NOFOLLOW,
                rustix::fs::Mode::empty(),
                rustix::fs::ResolveFlags::BENEATH
                    | rustix::fs::ResolveFlags::NO_SYMLINKS
                    | rustix::fs::ResolveFlags::NO_XDEV,
            )
            .map_err(std::io::Error::from)?,
        );
        let identity = platform_io::fs_safety::linux::FileIdentity::from_fd(&directory)?;
        directory.sync_all()?;
        parent.sync_all()?;
        self.check_running()?;
        Ok(identity)
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
        self.write_relative_atomic(&self.root.relative(path)?, bytes, replace)
    }

    fn write_relative_atomic(
        &self,
        path: &RelativePath,
        bytes: &[u8],
        replace: bool,
    ) -> Result<()> {
        let (parent, name) = path.split_parent();
        self.private_relative_dir(parent.as_ref())?;
        let diagnostic_parent = match &parent {
            Some(parent) => self.root.path.join(parent.as_path()),
            None => self.root.path.clone(),
        };
        let directory = PrivateDirectory::from_directory(
            self.open_relative_directory(parent.as_ref())?,
            diagnostic_parent,
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

    fn rename_noreplace_synced(
        &self,
        from: &Path,
        to: &Path,
        order: RenameSyncOrder,
    ) -> Result<()> {
        let (source, from_name) = self.private_parent(from)?;
        let (destination, to_name) = self.private_parent(to)?;
        self.check_running()?;
        rustix::fs::renameat_with(
            &source,
            from_name.as_os_str(),
            &destination,
            to_name.as_os_str(),
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)?;
        // Cancellation cannot skip a committed rename's durability tail.
        let (first, second) = match order {
            RenameSyncOrder::SourceFirst => (&source, &destination),
            RenameSyncOrder::DestinationFirst => (&destination, &source),
        };
        first.sync_all()?;
        if from.parent() != to.parent() {
            second.sync_all()?;
        }
        self.check_running()
    }

    fn remove_synced(&self, path: &Path, flags: rustix::fs::AtFlags) -> Result<()> {
        let (parent, name) = self.private_parent(path)?;
        self.check_running()?;
        rustix::fs::unlinkat(&parent, name.as_os_str(), flags).map_err(std::io::Error::from)?;
        parent.sync_all()?;
        self.check_running()
    }

    fn remove_owned_empty_directory_synced(
        &self,
        path: &Path,
        expected: platform_io::fs_safety::linux::FileIdentity,
    ) -> Result<()> {
        let (parent, name) = self.private_parent(path)?;
        let directory = rustix::fs::openat2(
            &parent,
            name.as_os_str(),
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::CLOEXEC
                | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
            rustix::fs::ResolveFlags::BENEATH
                | rustix::fs::ResolveFlags::NO_SYMLINKS
                | rustix::fs::ResolveFlags::NO_XDEV,
        )
        .map_err(std::io::Error::from)?;
        if platform_io::fs_safety::linux::FileIdentity::from_fd(&directory)? != expected {
            return Err(Error::new(
                ErrorCode::ContainerLayoutInvalid,
                "private layout preparation identity changed before cleanup",
            ));
        }
        self.check_running()?;
        // REMOVEDIR refuses nonempty directories. Never remove an unknown child.
        rustix::fs::unlinkat(&parent, name.as_os_str(), rustix::fs::AtFlags::REMOVEDIR)
            .map_err(std::io::Error::from)?;
        parent.sync_all()?;
        self.check_running()
    }

    fn remove_file_if_present_synced(&self, path: &Path) -> Result<bool> {
        let (parent, name) = self.private_parent(path)?;
        self.check_running()?;
        let removed =
            match rustix::fs::unlinkat(&parent, name.as_os_str(), rustix::fs::AtFlags::empty()) {
                Ok(()) => true,
                Err(rustix::io::Errno::NOENT) => false,
                Err(error) => return Err(std::io::Error::from(error).into()),
            };
        // Even an absent leaf can follow an earlier unlink whose sync failed.
        // Keep all sync errors distinct from unlink absence, including ENOENT.
        parent.sync_all()?;
        self.check_running()?;
        Ok(removed)
    }

    fn component_name_maximum(&self) -> Result<u64> {
        let stat = rustix::fs::fstatvfs(self.held().directory()).map_err(std::io::Error::from)?;
        Ok(stat.f_namemax)
    }

    fn available(&self) -> Result<u64> {
        let stat = rustix::fs::fstatvfs(self.held().directory()).map_err(std::io::Error::from)?;
        Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
    }

    fn lock_file(&self) -> Result<File> {
        let file = File::from(
            rustix::fs::openat2(
                self.held().directory(),
                layout::DISK_WRITER_LOCK.as_os_str(),
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
        let seal = identity_at(config, disk, root.operation()?.held().directory())?;
        if sealed {
            let bytes = root.read_bytes(
                &disk.root.join(layout::DISK_IDENTITY.as_path()),
                MAX_METADATA,
            )?;
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
        for disk in config.io_disks() {
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
        for disk in config.io_disks() {
            let root = DiskRoot::open(&disk.root)?;
            let seal = identity_at(config, disk, &root.directory(root.path())?)?;
            let bytes = root.read_bytes(
                &disk.root.join(layout::DISK_IDENTITY.as_path()),
                MAX_METADATA,
            )?;
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
            let identity = root.operation()?.held().identity();
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
            let seal = identity_at(config, disk, root.operation()?.held().directory())?;
            disks.insert(
                disk.disk_id.clone(),
                Arc::new(DiskHandle::new(root, seal, config)?),
            );
        }
        Ok(Self { disks })
    }

    pub(crate) fn recover_held(config: &ValidatedConfig, directories: Vec<File>) -> Result<Self> {
        if directories.len() != config.io_disks().count() {
            return Err(Error::new(
                ErrorCode::ConfigInvalid,
                "held recovery directory count differs from configuration",
            ));
        }
        let mut disks = BTreeMap::new();
        let mut identities = BTreeSet::new();
        for (disk, directory) in config.io_disks().zip(directories) {
            let root = DiskRoot::from_directory(disk.root.clone(), directory)?;
            let identity = root.operation()?.held().identity();
            if !identities.insert((identity.device, identity.inode)) {
                return Err(Error::new(
                    ErrorCode::ConfigInvalid,
                    "selected recovery directories name the same held storage root",
                ));
            }
            let seal = identity_at(config, disk, root.operation()?.held().directory())?;
            let bytes = root.read_bytes(
                &disk.root.join(layout::DISK_IDENTITY.as_path()),
                MAX_METADATA,
            )?;
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

    /// Unlike rejected recovery resources, lifecycle retirement requires the
    /// exact runtime closure proof. Historical config entries remain unchanged.
    pub(crate) fn excluding_closed(&self, closed: &ClosedDiskIo) -> Result<Self> {
        let disk_id = &closed.identity().disk_id;
        let handle = self.disks.get(disk_id).ok_or_else(|| {
            Error::new(
                ErrorCode::DiskIdentityMismatch,
                "closed disk is not among the admitted resources",
            )
        })?;
        closed.verify_binding(handle.root.io_binding()?)?;
        Ok(Self {
            disks: self
                .disks
                .iter()
                .filter(|(id, _)| *id != disk_id)
                .map(|(id, handle)| (id.clone(), Arc::clone(handle)))
                .collect(),
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
#[path = "../../../../test/engine/unit/disk/directory_retry_tests.rs"]
mod directory_retry_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/disk/root_operation_tests.rs"]
mod root_operation_tests;

#[cfg(test)]
#[path = "../../../../test/engine/unit/disk/directory_entries_tests.rs"]
mod directory_entries_tests;
