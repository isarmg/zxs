use crate::DiskRegistrationIdentity;
use crate::persist::layout;
use platform_io::fs_safety::{EntryName, linux::FileIdentity};
use std::collections::BTreeMap;
#[path = "admission.rs"]
pub(crate) mod admission;

pub(crate) use storage_contract::working_tree::{
    BindingLifecycle, ContainerBinding, DirectoryBinding, DirectoryId, LocalDirectoryBinding,
    LocalEntityIdentity, MaterialCommitment, ReplicaBinding, ReplicaId, ReplicaObject,
    ReplicaPurpose, RootBinding, RootId, SnapshotRepositoryBinding, SnapshotRepositoryId,
    SnapshotRepositoryLocation, StorageLayout, ThreeRootBinding, WorkingTreeCatalog,
};

pub(crate) fn local_entity(identity: FileIdentity) -> LocalEntityIdentity {
    LocalEntityIdentity::new(identity.device, identity.inode)
}
pub(crate) fn file_identity(identity: LocalEntityIdentity) -> FileIdentity {
    FileIdentity {
        device: identity.device(),
        inode: identity.inode(),
    }
}

/// Bounded structural closure only: ordering, names, parents, replicas and entities.
/// It proves neither authenticated origin nor completeness against FILE/route state.
/// No production activation or inventory classification may rely on this alone.
pub(crate) struct StructurallyValidWorkingTreeCatalog<'a> {
    catalog: &'a WorkingTreeCatalog,
    roots: BTreeMap<RootId, &'a RootBinding>,
    directories: BTreeMap<DirectoryId, &'a DirectoryBinding>,
    replicas: BTreeMap<ReplicaId, &'a ReplicaBinding>,
}
impl<'a> StructurallyValidWorkingTreeCatalog<'a> {
    pub(crate) fn catalog(&self) -> &'a WorkingTreeCatalog {
        self.catalog
    }
}

/// One structural root matched against an independently supplied current tuple.
/// This selection is not complete FILE authority or a permission to publish.
#[derive(Clone, Copy)]
pub(crate) struct CurrentRootSelection<'a> {
    tree: &'a StructurallyValidWorkingTreeCatalog<'a>,
    binding: &'a RootBinding,
}
impl<'a> CurrentRootSelection<'a> {
    pub(crate) fn binding(self) -> &'a RootBinding {
        self.binding
    }
    pub(crate) fn root_id(self) -> RootId {
        self.binding.root_id
    }
    pub(crate) fn registration(self) -> &'a DiskRegistrationIdentity {
        &self.binding.registration
    }
    pub(crate) fn common_data(self) -> &'a LocalDirectoryBinding {
        &self.binding.roots.common_data
    }
    /// Presence conservatively blocks protected traversal. Absence does not
    /// classify an observed entry as ordinary or prove an inventory complete.
    pub(crate) fn entry(
        self,
        parent: DirectoryId,
        name: &EntryName,
    ) -> Option<RegisteredEntry<'a>> {
        let key = name.as_os_str().to_str().map(name_policy::lookup_key)?;
        if let Some(directory) = self.tree.catalog.directories.iter().find(|row| {
            row.root_id == self.root_id()
                && row.parent_directory_id == parent
                && row.name.key() == key
        }) {
            return Some(RegisteredEntry::Directory(directory));
        }
        if let Some(container) = self.tree.catalog.containers.iter().find(|row| {
            row.root_id == self.root_id()
                && row.parent_directory_id == parent
                && row.name.key() == key
        }) {
            return Some(RegisteredEntry::Container(container));
        }
        let management = name_policy::lookup_key(
            &layout::SNAPSHOT_REPOSITORY.as_os_str().to_string_lossy(),
        );
        self.tree.catalog.snapshot_repositories.iter().find(|row| {
            row.root_id == self.root_id() && match &row.location {
                SnapshotRepositoryLocation::Attached => {
                    row.host_directory_id == parent && key == management
                }
                SnapshotRepositoryLocation::Retained { parent_directory_id, name, .. } => {
                    *parent_directory_id == parent && name.key() == key
                }
            }
        }).map(RegisteredEntry::SnapshotRepository)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum RegisteredEntry<'a> {
    Directory(&'a DirectoryBinding),
    Container(&'a ContainerBinding),
    SnapshotRepository(&'a SnapshotRepositoryBinding),
}
