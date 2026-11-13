//! Explicit local host capture. Neither a path nor a caller-provided inode is
//! publication authority; the original root and every held ancestor are checked.
use crate::disk::{DiskRoot, WorkingDirectory};
use crate::engine::snapshot_initialization::{failure, SnapshotInitializationError};
use crate::working_tree::admission::{
    ObservedDirectory, RootSelectionContext, register_directory_chain, select_root,
    validate_catalog_structure,
};
use crate::{Engine, Result};
use platform_io::fs_safety::PrivateDirectory;
use storage_contract::working_tree::{
    DirectoryBinding, DirectoryId, DirectoryTargetRequest, LocalEntityIdentity, RootBinding,
    WorkingTreeCatalog,
};

pub struct PreparedDirectorySnapshotTarget {
    pub(super) request: DirectoryTargetRequest,
    pub(super) root: RootBinding,
    pub(super) host: LocalEntityIdentity,
    pub(super) host_id: DirectoryId,
    pub(super) directories: Vec<DirectoryBinding>,
    pub(super) pins: Vec<DiskRoot>,
    // Retain the caller's original capability as well as the registered walk.
    pub(super) explicit_host: Option<PrivateDirectory>,
}

fn rejected() -> crate::Error {
    failure(SnapshotInitializationError::Registration)
}

impl Engine {
    /// Trusted local capture only. The caller must authorize the explicit
    /// logical-source/physical-target correspondence. No namespace path is
    /// inferred from a FILE source; HTTP and SMB do not acquire authority here.
    /// This performs no filesystem writes and creates no repository or ledger.
    pub fn prepare_directory_snapshot_target(
        &self,
        request: DirectoryTargetRequest,
        host: PrivateDirectory,
    ) -> Result<PreparedDirectorySnapshotTarget> {
        self.capture_directory_snapshot_target(request, Some(host))
    }

    pub(super) fn capture_directory_snapshot_target(
        &self,
        request: DirectoryTargetRequest,
        explicit_host: Option<PrivateDirectory>,
    ) -> Result<PreparedDirectorySnapshotTarget> {
        let admitted = self.fresh_directory_root(request.root_id)?;
        let root = admitted.binding().clone();
        let mut known = std::collections::BTreeMap::new();
        for registered in self.directory_snapshots.values() {
            for directory in &registered.record.intent.directories {
                if directory.root_id == root.root_id {
                    if let Some(previous) = known.insert(directory.directory_id, directory.clone())
                        && previous != *directory
                    {
                        return Err(rejected());
                    }
                }
            }
        }
        let catalog = WorkingTreeCatalog {
            storage_layout: root.layout,
            roots: vec![root.clone()],
            directories: known.into_values().collect(),
            containers: Vec::new(),
            snapshot_repositories: Vec::new(),
            replicas: Vec::new(),
        };
        let view = validate_catalog_structure(&catalog).map_err(|_| rejected())?;
        let context = RootSelectionContext {
            storage_layout: root.layout,
            registration: &root.registration,
            source_chain: &root.source_chain,
        };
        let selected = select_root(&view, root.root_id, &context).map_err(|_| rejected())?;
        let disk = self.disk_resources.get(self.disk(&root.registration.slot_id)?)?;
        let held = WorkingDirectory::open(disk, selected, &request.path)
            .map_err(|_| rejected())?;
        let identity = held.identity().map_err(|_| rejected())?;
        if let Some(explicit) = &explicit_host {
            explicit.verify_identity()?;
            let file = explicit.try_clone()?;
            let file = platform_io::fs_safety::linux::OpenAt2Root::from_private_directory(
                file, platform_io::fs_safety::linux::MountPolicy::SameMount,
            )?;
            let expected = platform_io::fs_safety::linux::FileIdentity::from_fd(file.directory())?;
            if identity != expected {
                return Err(rejected());
            }
        }
        let observations: Vec<_> = held.observed_chain().map(|entry| ObservedDirectory {
            name: entry.name.clone(), entity: entry.entity,
        }).collect();
        let chain = register_directory_chain(selected, &request, &observations)
            .map_err(|_| rejected())?;
        let mut parent = root.roots.common_data.directory_id;
        let mut directories = Vec::new();
        for observed in &observations {
            let binding = catalog.directories.iter().chain(&chain.additions).find(|binding| {
                binding.parent_directory_id == parent
                    && binding.name.as_str() == observed.name.as_os_str().to_str().unwrap_or("")
            }).ok_or_else(rejected)?;
            parent = binding.directory_id;
            directories.push(binding.clone());
        }
        let pins = held.retain_chain().map_err(|_| rejected())?;
        held.revalidate().map_err(|_| rejected())?;
        Ok(PreparedDirectorySnapshotTarget {
            request,
            root,
            host: LocalEntityIdentity::new(identity.device, identity.inode),
            host_id: chain.host_directory_id,
            directories,
            pins,
            explicit_host,
        })
    }
}
