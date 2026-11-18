//! One physical creation/readback kernel for initializer and fresh Add areas.
//! The caller supplies its actual exclusive parent and owns its writer fence.
//! These retained descriptors do not establish membership or select a root.
use crate::engine::fresh_layout::{FreshLayoutError as Failure, directory_error};
use crate::disk::{DirectoryCreateOutcome, DiskRoot};
use crate::persist::layout;
use crate::namespace_roots::{file_identity, local_entity};
use crate::Result;
use platform_io::fs_safety::EntryName;
use storage_contract::namespace_root::{
    DirectoryId, LocalDirectoryBinding, ThreeRootBinding, ZxsLifecycleBindings,
};

pub(in crate::engine) struct HeldRootAreas {
    common: DiskRoot,
    deleted: DiskRoot,
    management: DiskRoot,
    current: DiskRoot,
    retained: DiskRoot,
    binding: ThreeRootBinding,
}
impl HeldRootAreas {
    pub(in crate::engine) fn create(parent: &DiskRoot, ids: [DirectoryId; 5]) -> Result<Self> {
        let (common, common_data) = create(parent, &layout::COMMON_DATA_ROOT, ids[0])?;
        let (deleted, deleted_data) = create(parent, &layout::DELETED_DATA_ROOT, ids[1])?;
        let (management, zxs_data) = create(parent, &layout::ZXS_DATA_ROOT, ids[2])?;
        let (current, current_binding) = create(&management, &layout::COMMON_DATA_ROOT, ids[3])?;
        let (retained, retained_binding) = create(&management, &layout::DELETED_DATA_ROOT, ids[4])?;
        management.sync_dir(management.path())?;
        parent.sync_dir(parent.path())?;
        let result = Self {
            common, deleted, management, current, retained,
            binding: ThreeRootBinding {
                common_data, deleted_data, zxs_data,
                zxs_lifecycles: ZxsLifecycleBindings {
                    common_data: current_binding, deleted_data: retained_binding,
                },
            },
        };
        result.verify(parent)?;
        Ok(result)
    }

    pub(in crate::engine) fn observe(
        parent: &DiskRoot, binding: &ThreeRootBinding,
    ) -> Result<Self> {
        let common = open(parent, &layout::COMMON_DATA_ROOT, &binding.common_data)?;
        let deleted = open(parent, &layout::DELETED_DATA_ROOT, &binding.deleted_data)?;
        let management = open(parent, &layout::ZXS_DATA_ROOT, &binding.zxs_data)?;
        Self::from_top(common, deleted, management, binding)
    }

    /// A completed immutable feedback record supplies every identity. Exactly
    /// one of the original stage name and the final name must contain each root.
    /// This observes partial installation; it never creates or adopts an area.
    pub(in crate::engine) fn observe_installation(
        root: &DiskRoot, stage: Option<&DiskRoot>, binding: &ThreeRootBinding,
    ) -> Result<Self> {
        let select = |name: &EntryName, expected: &LocalDirectoryBinding| -> Result<DiskRoot> {
            let final_exists = root.optional_metadata(&root.path().join(name.as_path()))?.is_some();
            let staged = stage.map(|stage| {
                stage.optional_metadata(&stage.path().join(name.as_path()))
                    .map(|entry| entry.is_some())
            }).transpose()?.unwrap_or(false);
            match (final_exists, stage, staged) {
                (true, _, false) => open(root, name, expected),
                (false, Some(stage), true) => open(stage, name, expected),
                _ => Err(Failure::AdditionEvidence.into()),
            }
        };
        let common = select(&layout::COMMON_DATA_ROOT, &binding.common_data)?;
        let deleted = select(&layout::DELETED_DATA_ROOT, &binding.deleted_data)?;
        let management = select(&layout::ZXS_DATA_ROOT, &binding.zxs_data)?;
        Self::from_top(common, deleted, management, binding)
    }

    fn from_top(
        common: DiskRoot, deleted: DiskRoot, management: DiskRoot,
        binding: &ThreeRootBinding,
    ) -> Result<Self> {
        let current = open(&management, &layout::COMMON_DATA_ROOT,
            &binding.zxs_lifecycles.common_data)?;
        let retained = open(&management, &layout::DELETED_DATA_ROOT,
            &binding.zxs_lifecycles.deleted_data)?;
        Ok(Self { common, deleted, management, current, retained, binding: binding.clone() })
    }

    /// Pending additions may contain control metadata only outside these areas.
    /// A file placed inside a newly created data/lifecycle root is not adopted.
    pub(in crate::engine) fn require_empty(&self) -> Result<()> {
        for root in [&self.common, &self.deleted, &self.current, &self.retained] {
            if root.entries(root.path())?.next().transpose().map_err(crate::disk::fs_error)?
                .is_some()
            {
                return Err(Failure::AdditionEvidence.into());
            }
        }
        for entry in self.management.entries(self.management.path())? {
            let entry = entry.map_err(crate::disk::fs_error)?;
            if entry != *layout::COMMON_DATA_ROOT && entry != *layout::DELETED_DATA_ROOT {
                return Err(Failure::AdditionEvidence.into());
            }
        }
        Ok(())
    }

    pub(in crate::engine) fn binding(&self) -> &ThreeRootBinding {
        &self.binding
    }

    pub(in crate::engine) fn verify(&self, parent: &DiskRoot) -> Result<()> {
        for (name, held, expected) in [
            (&*layout::COMMON_DATA_ROOT, &self.common, &self.binding.common_data),
            (&*layout::DELETED_DATA_ROOT, &self.deleted, &self.binding.deleted_data),
            (&*layout::ZXS_DATA_ROOT, &self.management, &self.binding.zxs_data),
        ] {
            require(parent, name, held, expected)?;
        }
        require(&self.management, &layout::COMMON_DATA_ROOT, &self.current,
            &self.binding.zxs_lifecycles.common_data)?;
        require(&self.management, &layout::DELETED_DATA_ROOT, &self.retained,
            &self.binding.zxs_lifecycles.deleted_data)
    }
}

fn create(
    parent: &DiskRoot, name: &EntryName, directory_id: DirectoryId,
) -> Result<(DiskRoot, LocalDirectoryBinding)> {
    match parent.create_private_entry(name) {
        DirectoryCreateOutcome::Created { directory, identity, durability } => {
            durability.map_err(directory_error)?;
            if directory.directory_identity(directory.path())? != identity {
                return Err(Failure::RootIdentity.into());
            }
            parent.sync_private_entry_matching(name, identity).map_err(directory_error)?;
            Ok((directory, LocalDirectoryBinding { directory_id, entity: local_entity(identity) }))
        }
        DirectoryCreateOutcome::Rejected(error)
        | DirectoryCreateOutcome::Uncertain { source: error, .. } => Err(directory_error(error)),
    }
}

fn open(parent: &DiskRoot, name: &EntryName, expected: &LocalDirectoryBinding) -> Result<DiskRoot> {
    parent.private_entry_matching(name, file_identity(expected.entity)).map_err(directory_error)
}

fn require(
    parent: &DiskRoot, name: &EntryName, held: &DiskRoot, expected: &LocalDirectoryBinding,
) -> Result<()> {
    let named = open(parent, name, expected)?;
    if held.directory_identity(held.path())? != named.directory_identity(named.path())? {
        return Err(Failure::RootIdentity.into());
    }
    Ok(())
}
