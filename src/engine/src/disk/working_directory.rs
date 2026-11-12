//! Ordinary directory access is separate from private storage administration.
//! Every retained descriptor belongs to the original disk's drain domain.
use crate::disk::owned::{DiskDirectoryEntries, DiskHandle, DiskRoot, RootOperation};
use crate::persist::layout;
use crate::working_tree::{CurrentRootSelection, RegisteredEntry, records::file_identity};
use platform_io::fs_safety::{
    EntryName, PrivateDirectory,
    linux::{FileIdentity, MountPolicy, OpenAt2Root},
};
use rustix::fs::{Mode, OFlags, RenameFlags, ResolveFlags};
use std::{fs::File, io, os::unix::fs::MetadataExt, sync::Arc};
use storage_contract::working_tree::{DirectoryId, DirectoryTargetPath};

type IoResult<T> = std::result::Result<T, DirectoryIoError>;

const MAX_DIRECTORY_PROBE_ENTRIES: usize = 100_000;

#[derive(Debug, error_definition::Error)]
pub(crate) enum DirectoryIoError {
    #[error("disk admission refused directory I/O: {0}")]
    Gate(#[source] crate::Error),
    #[error("directory filesystem policy refused access: {0}")]
    Filesystem(#[source] platform_io::fs_safety::Error),
    #[error("directory I/O failed: {0}")]
    Io(#[source] io::Error),
    #[error("directory does not belong to the admitted working root")]
    WrongRoot,
    #[error("directory entry changed: {0:?}")]
    IdentityChanged(EntryName),
    #[error("working traversal enters a registered protected object: {0:?}")]
    ProtectedEntry(EntryName),
    #[error("directory publication target already exists: {0:?}")]
    Collision(EntryName),
    #[error("directory publication crosses disk I/O domains or filesystems")]
    WrongFilesystem,
    #[error("directory inventory exceeds its admitted entry budget")]
    InventoryBudget,
    #[error("private directory ownership or mode changed")]
    NotPrivate,
    #[error("directory synchronization failed: {first}; additional failure: {second:?}")]
    Sync {
        #[source]
        first: io::Error,
        second: Option<io::Error>,
    },
    #[error("{source}; completing the durability tail also failed: {tail}")]
    Tail {
        #[source]
        source: Box<DirectoryIoError>,
        tail: Box<DirectoryIoError>,
    },
}

/// Even an errno after an attempted mkdir can require same-operation recovery.
/// A returned identity was observed after mkdir, not atomically returned by it.
pub(crate) enum DirectoryCreateOutcome {
    Rejected(DirectoryIoError),
    Uncertain {
        identity: Option<FileIdentity>,
        source: DirectoryIoError,
    },
    Created {
        directory: DiskRoot,
        identity: FileIdentity,
        durability: IoResult<()>,
    },
}

/// A successful rename is never reported as an ordinary pre-effect failure.
pub(crate) enum DirectoryInstallOutcome {
    Rejected(DirectoryIoError),
    Uncertain(DirectoryIoError),
    Installed {
        // Intended prepared entity, not an admission receipt when durability is Err.
        identity: FileIdentity,
        durability: IoResult<()>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RepositoryLeaf {
    Absent,
    Occupied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkingDirectoryIdentity {
    pub(crate) name: EntryName,
    pub(crate) entity: FileIdentity,
}

struct HeldAncestor {
    observed: WorkingDirectoryIdentity,
    directory: DiskRoot,
}

/// This value cannot yield a DiskRoot or raw descriptor for the ordinary host.
/// The borrowed selection fixes structural records, not complete FILE authority.
/// Production integration must supply its authenticated complete object inventory.
pub(crate) struct WorkingDirectory<'a> {
    selected: CurrentRootSelection<'a>,
    disk: DiskRoot,
    common: DiskRoot,
    ancestors: Vec<HeldAncestor>,
    host_id: Option<DirectoryId>,
}

fn retain_root(root: &DiskRoot) -> DiskRoot {
    DiskRoot {
        path: root.path.clone(),
        directory: Arc::clone(&root.directory),
        io: root.binding().cloned(),
    }
}

impl<'a> WorkingDirectory<'a> {
    pub(crate) fn open(
        disk: &DiskHandle,
        selected: CurrentRootSelection<'a>,
        path: &DirectoryTargetPath,
    ) -> IoResult<Self> {
        let lease = disk.root.admit_io().map_err(DirectoryIoError::Gate)?;
        let registration = selected.registration();
        if registration.pool_id != disk.seal.pool_id
            || registration.disk_id != disk.seal.disk_id
            || registration.slot_id != disk.seal.slot_id
            || registration.binding_generation != disk.seal.binding_generation
            || disk.root.held_identity()? != file_identity(selected.binding().root_entity)
        {
            return Err(DirectoryIoError::WrongRoot);
        }
        let common = disk.root.private_entry_matching(
            &layout::COMMON_DATA_ROOT,
            file_identity(selected.common_data().entity),
        )?;
        let mut held = Self {
            host_id: Some(selected.common_data().directory_id),
            selected,
            disk: retain_root(&disk.root),
            common,
            ancestors: Vec::new(),
        };
        if let DirectoryTargetPath::Relative(path) = path {
            for component in path.components() {
                lease.check_running().map_err(DirectoryIoError::Gate)?;
                let name = EntryName::new(component.as_str())
                    .map_err(DirectoryIoError::Filesystem)?;
                held.open_descendant(name)?;
            }
        }
        held.revalidate()?;
        lease.check_running().map_err(DirectoryIoError::Gate)?;
        Ok(held)
    }

    fn host(&self) -> &DiskRoot {
        match self.ancestors.last() {
            Some(ancestor) => &ancestor.directory,
            None => &self.common,
        }
    }

    fn open_descendant(&mut self, name: EntryName) -> IoResult<()> {
        // Unknown management-shaped entries remain conflicts, not authority.
        if repository_spelling(&name) {
            return Err(DirectoryIoError::ProtectedEntry(name));
        }
        let (registered, expected) = match self.host_id {
            Some(parent) => match self.selected.entry(parent, &name) {
                Some(RegisteredEntry::Directory(directory)) => {
                    if name.as_os_str().to_str() != Some(directory.name.as_str()) {
                        return Err(DirectoryIoError::IdentityChanged(name));
                    }
                    (Some(directory.directory_id), Some(file_identity(directory.entity)))
                }
                Some(RegisteredEntry::Container(_) | RegisteredEntry::SnapshotRepository(_)) => {
                    return Err(DirectoryIoError::ProtectedEntry(name));
                }
                None => (None, None),
            },
            None => (None, None),
        };
        let parent = self.host();
        let operation = parent.operation().map_err(DirectoryIoError::Gate)?;
        let directory = open_entry(operation.held().directory(), &name)?;
        let entity = FileIdentity::from_fd(&directory).map_err(DirectoryIoError::Io)?;
        if expected.is_some_and(|expected| expected != entity) {
            return Err(DirectoryIoError::IdentityChanged(name));
        }
        let directory = OpenAt2Root::from_directory(directory, MountPolicy::SameMount)
            .map_err(DirectoryIoError::Filesystem)?;
        let directory = DiskRoot::held(
            parent.path.join(name.as_path()),
            directory,
            parent.binding().cloned(),
        )
        .map_err(DirectoryIoError::Gate)?;
        drop(operation);
        self.ancestors.push(HeldAncestor {
            observed: WorkingDirectoryIdentity { name, entity },
            directory,
        });
        self.host_id = registered;
        Ok(())
    }

    pub(crate) fn identity(&self) -> IoResult<FileIdentity> {
        self.host().held_identity()
    }

    pub(crate) fn root_identity(&self) -> IoResult<FileIdentity> {
        self.common.held_identity()
    }

    pub(crate) fn registered_host_id(&self) -> Option<DirectoryId> {
        self.host_id
    }

    pub(crate) fn observed_chain(
        &self,
    ) -> impl ExactSizeIterator<Item = &WorkingDirectoryIdentity> {
        self.ancestors.iter().map(|ancestor| &ancestor.observed)
    }

    /// A held FD is not a namespace lock. Replacements fence the original
    /// operation; they are never followed as the same registered directory.
    pub(crate) fn revalidate(&self) -> IoResult<()> {
        let disk = self.disk.operation().map_err(DirectoryIoError::Gate)?;
        verify_entry(
            disk.held().directory(),
            &layout::COMMON_DATA_ROOT,
            self.common.held_identity()?,
        )?;
        let common = self.common.operation().map_err(DirectoryIoError::Gate)?;
        require_private(common.held().directory())?;
        let mut parent = &self.common;
        for ancestor in &self.ancestors {
            let operation = parent.operation().map_err(DirectoryIoError::Gate)?;
            verify_entry(
                operation.held().directory(),
                &ancestor.observed.name,
                ancestor.directory.held_identity()?,
            )?;
            parent = &ancestor.directory;
        }
        disk.check_running().map_err(DirectoryIoError::Gate)
    }

    pub(crate) fn probe_repository(&self, maximum_entries: usize) -> IoResult<RepositoryLeaf> {
        self.revalidate()?;
        let operation = self.host().operation().map_err(DirectoryIoError::Gate)?;
        let result = probe_repository(&operation, maximum_entries)?;
        self.revalidate()?;
        operation.check_running().map_err(DirectoryIoError::Gate)?;
        Ok(result)
    }

    /// Publication is authorized by the caller's original permanent decision.
    /// This mechanism only installs its exact held prepared entity; it does not
    /// turn the stage's name, marker or inode number into a repository binding.
    pub(crate) fn install_repository(
        &self,
        source_parent: &DiskRoot,
        source_name: &EntryName,
        prepared: &DiskRoot,
        maximum_entries: usize,
    ) -> DirectoryInstallOutcome {
        match self.install_admitted(source_parent, source_name, prepared, maximum_entries) {
            Ok(outcome) => outcome,
            Err(source) => DirectoryInstallOutcome::Rejected(source),
        }
    }

    fn install_admitted(
        &self,
        source_parent: &DiskRoot,
        source_name: &EntryName,
        prepared: &DiskRoot,
        maximum_entries: usize,
    ) -> IoResult<DirectoryInstallOutcome> {
        let disk_binding = self.disk.io_binding().map_err(DirectoryIoError::Gate)?;
        for root in [source_parent, prepared] {
            if !disk_binding.same_domain(root.io_binding().map_err(DirectoryIoError::Gate)?) {
                return Err(DirectoryIoError::WrongFilesystem);
            }
        }
        let source = source_parent.operation().map_err(DirectoryIoError::Gate)?;
        let payload = prepared.operation().map_err(DirectoryIoError::Gate)?;
        let destination = self.host().operation().map_err(DirectoryIoError::Gate)?;
        require_private(source.held().directory())?;
        require_private(payload.held().directory())?;
        self.verify_source_parent(source_parent)?;
        self.revalidate()?;
        let identity = payload.held().identity();
        if identity.device != destination.held().identity().device {
            return Err(DirectoryIoError::WrongFilesystem);
        }
        verify_entry(source.held().directory(), source_name, identity)?;
        if probe_repository(&destination, maximum_entries)? == RepositoryLeaf::Occupied {
            return Err(DirectoryIoError::Collision(layout::SNAPSHOT_REPOSITORY.clone()));
        }
        self.revalidate()?;
        self.verify_source_parent(source_parent)?;
        verify_entry(source.held().directory(), source_name, identity)?;
        source.check_running().map_err(DirectoryIoError::Gate)?;
        destination.check_running().map_err(DirectoryIoError::Gate)?;
        match rustix::fs::renameat_with(
            source.held().directory(),
            source_name.as_os_str(),
            destination.held().directory(),
            layout::SNAPSHOT_REPOSITORY.as_os_str(),
            RenameFlags::NOREPLACE,
        ) {
            Ok(()) => (),
            Err(rustix::io::Errno::EXIST) => {
                return Ok(DirectoryInstallOutcome::Rejected(DirectoryIoError::Collision(
                    layout::SNAPSHOT_REPOSITORY.clone(),
                )));
            }
            Err(error) => {
                let source = preserve_tail(
                    DirectoryIoError::Io(error.into()),
                    sync_pair(destination.held().directory(), source.held().directory()),
                );
                return Ok(DirectoryInstallOutcome::Uncertain(source));
            }
        }
        // Once renamed, cancellation cannot skip either parent's durability tail.
        let durability = sync_pair(destination.held().directory(), source.held().directory())
            .and_then(|()| require_private(source.held().directory()))
            .and_then(|()| require_private(payload.held().directory()))
            .and_then(|()| self.revalidate())
            .and_then(|()| self.verify_source_parent(source_parent))
            .and_then(|()| {
                verify_entry(
                    destination.held().directory(),
                    &layout::SNAPSHOT_REPOSITORY,
                    identity,
                )
            })
            .and_then(|()| reject_repository_alias(&destination, maximum_entries))
            .and_then(|()| destination.check_running().map_err(DirectoryIoError::Gate));
        Ok(DirectoryInstallOutcome::Installed { identity, durability })
    }

    fn verify_source_parent(&self, parent: &DiskRoot) -> IoResult<()> {
        let root = self.disk.private_entry_matching(
            &layout::ZXS_DATA_ROOT,
            file_identity(self.selected.binding().roots.zxs_data.entity),
        )?;
        let operation = root.operation().map_err(DirectoryIoError::Gate)?;
        let expected = parent.held_identity()?;
        if parent.path == root.path {
            if operation.held().identity() != expected {
                return Err(DirectoryIoError::WrongRoot);
            }
            return Ok(());
        }
        let relative = root.relative(&parent.path).map_err(DirectoryIoError::Gate)?;
        let named = operation
            .held()
            .open_directory(&relative)
            .map_err(DirectoryIoError::Filesystem)?;
        if FileIdentity::from_fd(&named).map_err(DirectoryIoError::Io)? != expected {
            return Err(DirectoryIoError::WrongRoot);
        }
        Ok(())
    }
}

fn repository_spelling(name: &EntryName) -> bool {
    name.as_os_str().to_str().is_some_and(|name| {
        name_policy::lookup_key(name)
            == name_policy::lookup_key(
                layout::SNAPSHOT_REPOSITORY
                    .as_os_str()
                    .to_str()
                    .expect("invariant: repository layout entry is ASCII"),
            )
    })
}

fn probe_repository(
    operation: &RootOperation<'_>,
    maximum_entries: usize,
) -> IoResult<RepositoryLeaf> {
    // O_PATH | NOFOLLOW observes a symlink itself, including a dangling link.
    match rustix::fs::openat2(
        operation.held().directory(),
        layout::SNAPSHOT_REPOSITORY.as_os_str(),
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    ) {
        Ok(_) => return Ok(RepositoryLeaf::Occupied),
        Err(rustix::io::Errno::NOENT) => (),
        Err(error) => return Err(DirectoryIoError::Io(error.into())),
    }
    let entries = operation.held().entries().map_err(DirectoryIoError::Filesystem)?;
    for (count, name) in entries.enumerate() {
        operation.check_running().map_err(DirectoryIoError::Gate)?;
        if count == maximum_entries.min(MAX_DIRECTORY_PROBE_ENTRIES) {
            return Err(DirectoryIoError::InventoryBudget);
        }
        if repository_spelling(&name.map_err(DirectoryIoError::Filesystem)?) {
            return Ok(RepositoryLeaf::Occupied);
        }
    }
    Ok(RepositoryLeaf::Absent)
}

fn reject_repository_alias(
    operation: &RootOperation<'_>,
    maximum_entries: usize,
) -> IoResult<()> {
    // The successful rename added one fixed management entry, not another
    // ordinary inventory item. Its own slot must not exhaust a preflight budget.
    let maximum = maximum_entries.min(MAX_DIRECTORY_PROBE_ENTRIES).saturating_add(1);
    let entries = operation.held().entries().map_err(DirectoryIoError::Filesystem)?;
    for (count, name) in entries.enumerate() {
        operation.check_running().map_err(DirectoryIoError::Gate)?;
        if count == maximum {
            return Err(DirectoryIoError::InventoryBudget);
        }
        let name = name.map_err(DirectoryIoError::Filesystem)?;
        if name != *layout::SNAPSHOT_REPOSITORY && repository_spelling(&name) {
            return Err(DirectoryIoError::Collision(name));
        }
    }
    Ok(())
}

fn preserve_tail(source: DirectoryIoError, tail: IoResult<()>) -> DirectoryIoError {
    match tail {
        Ok(()) => source,
        Err(tail) => DirectoryIoError::Tail {
            source: Box::new(source),
            tail: Box::new(tail),
        },
    }
}

fn sync_pair(first: &File, second: &File) -> IoResult<()> {
    // Evaluate both barriers before interpreting either result.
    let first = first.sync_all();
    let second = second.sync_all();
    match (first, second) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(first), second) => Err(DirectoryIoError::Sync {
            first,
            second: second.err(),
        }),
        (Ok(()), Err(first)) => Err(DirectoryIoError::Sync {
            first,
            second: None,
        }),
    }
}

fn open_entry(parent: &File, name: &EntryName) -> IoResult<File> {
    rustix::fs::openat2(
        parent,
        name.as_os_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )
    .map(File::from)
    .map_err(|source| DirectoryIoError::Io(source.into()))
}

fn verify_entry(parent: &File, name: &EntryName, held: FileIdentity) -> IoResult<()> {
    let named = open_entry(parent, name)?;
    if FileIdentity::from_fd(&named).map_err(DirectoryIoError::Io)? != held {
        return Err(DirectoryIoError::IdentityChanged(name.clone()));
    }
    Ok(())
}

fn require_private(directory: &File) -> IoResult<()> {
    let metadata = directory.metadata().map_err(DirectoryIoError::Io)?;
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(DirectoryIoError::NotPrivate);
    }
    Ok(())
}

impl DiskRoot {
    pub(crate) fn held_identity(&self) -> IoResult<FileIdentity> {
        Ok(self.operation().map_err(DirectoryIoError::Gate)?.held().identity())
    }

    pub(crate) fn root_entries(&self) -> IoResult<DiskDirectoryEntries> {
        let operation = self.operation().map_err(DirectoryIoError::Gate)?;
        let entries = operation.held().entries().map_err(DirectoryIoError::Filesystem)?;
        let RootOperation { directory, lease, .. } = operation;
        drop(directory);
        DiskDirectoryEntries::new(entries, lease).map_err(DirectoryIoError::Gate)
    }

    pub(crate) fn private_entry_matching(
        &self,
        name: &EntryName,
        expected: FileIdentity,
    ) -> IoResult<Self> {
        let operation = self.operation().map_err(DirectoryIoError::Gate)?;
        let directory = open_entry(operation.held().directory(), name)?;
        if FileIdentity::from_fd(&directory).map_err(DirectoryIoError::Io)? != expected {
            return Err(DirectoryIoError::IdentityChanged(name.clone()));
        }
        let path = self.path.join(name.as_path());
        let private = PrivateDirectory::from_directory(directory, path.clone())
            .map_err(DirectoryIoError::Filesystem)?;
        let directory = OpenAt2Root::from_private_directory(private, MountPolicy::SameMount)
            .map_err(DirectoryIoError::Filesystem)?;
        Self::held(path, directory, self.binding().cloned()).map_err(DirectoryIoError::Gate)
    }

    pub(crate) fn sync_private_entry_matching(
        &self,
        name: &EntryName,
        expected: FileIdentity,
    ) -> IoResult<()> {
        let operation = self.operation().map_err(DirectoryIoError::Gate)?;
        let directory = open_entry(operation.held().directory(), name)?;
        if FileIdentity::from_fd(&directory).map_err(DirectoryIoError::Io)? != expected {
            return Err(DirectoryIoError::IdentityChanged(name.clone()));
        }
        require_private(&directory)?;
        sync_pair(&directory, operation.held().directory())?;
        verify_entry(operation.held().directory(), name, expected)?;
        operation.check_running().map_err(DirectoryIoError::Gate)
    }

    /// The original operation must exclusively own this private parent under
    /// its writer fence. Linux mkdirat returns no FD: a same-UID actor replacing
    /// the name before the first open cannot be ruled out by inode checks.
    /// Neither this outcome nor a marker grants ownership or recovery authority.
    pub(crate) fn create_private_entry(&self, name: &EntryName) -> DirectoryCreateOutcome {
        let operation = match self.operation() {
            Ok(operation) => operation,
            Err(source) => return DirectoryCreateOutcome::Rejected(DirectoryIoError::Gate(source)),
        };
        if let Err(source) = require_private(operation.held().directory()) {
            return DirectoryCreateOutcome::Rejected(source);
        }
        if let Err(source) = operation.check_running() {
            return DirectoryCreateOutcome::Rejected(DirectoryIoError::Gate(source));
        }
        match rustix::fs::mkdirat(
            operation.held().directory(),
            name.as_os_str(),
            Mode::from_raw_mode(0o700),
        ) {
            Ok(()) => self.finish_private_creation(name, &operation),
            Err(rustix::io::Errno::EXIST) => {
                DirectoryCreateOutcome::Rejected(DirectoryIoError::Collision(name.clone()))
            }
            Err(source) => {
                let tail = operation.held().directory().sync_all().map_err(DirectoryIoError::Io);
                DirectoryCreateOutcome::Uncertain {
                    identity: None,
                    source: preserve_tail(DirectoryIoError::Io(source.into()), tail),
                }
            }
        }
    }

    fn finish_private_creation(
        &self,
        name: &EntryName,
        operation: &RootOperation<'_>,
    ) -> DirectoryCreateOutcome {
        let directory = match open_entry(operation.held().directory(), name) {
            Ok(directory) => directory,
            Err(source) => {
                let tail = operation.held().directory().sync_all().map_err(DirectoryIoError::Io);
                return DirectoryCreateOutcome::Uncertain {
                    identity: None,
                    source: preserve_tail(source, tail),
                };
            }
        };
        let identity = match FileIdentity::from_fd(&directory) {
            Ok(identity) => identity,
            Err(source) => {
                return DirectoryCreateOutcome::Uncertain {
                    identity: None,
                    source: preserve_tail(
                        DirectoryIoError::Io(source),
                        sync_pair(&directory, operation.held().directory()),
                    ),
                };
            }
        };
        let durability = sync_pair(&directory, operation.held().directory())
            .and_then(|()| require_private(operation.held().directory()))
            .and_then(|()| verify_entry(operation.held().directory(), name, identity))
            .and_then(|()| operation.check_running().map_err(DirectoryIoError::Gate));
        let path = self.path.join(name.as_path());
        let held = PrivateDirectory::from_directory(directory, path.clone())
            .map_err(DirectoryIoError::Filesystem)
            .and_then(|directory| {
                OpenAt2Root::from_private_directory(directory, MountPolicy::SameMount)
                    .map_err(DirectoryIoError::Filesystem)
            })
            .and_then(|directory| {
                Self::held(path, directory, self.binding().cloned()).map_err(DirectoryIoError::Gate)
            });
        match held {
            Ok(directory) => DirectoryCreateOutcome::Created { directory, identity, durability },
            Err(source) => DirectoryCreateOutcome::Uncertain {
                identity: Some(identity),
                source: preserve_tail(source, durability),
            },
        }
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/disk/working_directory_tests.rs"]
mod tests;
