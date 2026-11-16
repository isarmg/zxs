//! Every child belongs to this exact exclusive source preparation. No missing
//! child is repaired or adopted during observation of a decided/published scope.
use crate::container_storage::ordinary_preparation::prefix::create_exclusive;
use crate::disk::DiskRoot;
use crate::engine::fresh_layout::FreshLayoutError as Failure;
use crate::persist::layout;
use crate::Result;
use platform_io::fs_safety::{EntryName, linux::FileIdentity};
use std::sync::Arc;
use storage_contract::file_container::{
    ContainerAreas, ContainerEntity, ContainerManagementAreas,
};

pub(super) struct HeldAreas {
    common: Arc<DiskRoot>,
    deleted: Arc<DiskRoot>,
    management: Arc<DiskRoot>,
    current: Arc<DiskRoot>,
    retained: Arc<DiskRoot>,
    record: ContainerAreas,
}
impl HeldAreas {
    pub(super) fn create(parent: &DiskRoot) -> Result<Self> {
        let create = |root: &DiskRoot, name: &EntryName|
            -> Result<(Arc<DiskRoot>, ContainerEntity)> {
            let (directory, entity) = create_exclusive(root, name)?;
            Ok((Arc::new(directory), ContainerEntity::new(entity.device, entity.inode)))
        };
        let (common, common_entity) = create(parent, &layout::COMMON_DATA_ROOT)?;
        let (deleted, deleted_entity) = create(parent, &layout::DELETED_DATA_ROOT)?;
        let (management, management_entity) = create(parent, &layout::ZXS_DATA_ROOT)?;
        let (current, current_entity) = create(&management, &layout::COMMON_DATA_ROOT)?;
        let (retained, retained_entity) = create(&management, &layout::DELETED_DATA_ROOT)?;
        management.sync_dir(management.path())?;
        parent.sync_dir(parent.path())?;
        let result = Self {
            common, deleted, management, current, retained,
            record: ContainerAreas {
                common_data: common_entity, deleted_data: deleted_entity,
                zxs_data: ContainerManagementAreas {
                    root: management_entity,
                    common_data: current_entity, deleted_data: retained_entity,
                },
            },
        };
        result.verify(parent)?;
        Ok(result)
    }

    pub(super) fn observe(parent: &DiskRoot, record: ContainerAreas) -> Result<Self> {
        let common = open(parent, &layout::COMMON_DATA_ROOT, record.common_data)?;
        let deleted = open(parent, &layout::DELETED_DATA_ROOT, record.deleted_data)?;
        let management = open(parent, &layout::ZXS_DATA_ROOT, record.zxs_data.root)?;
        let current = open(&management, &layout::COMMON_DATA_ROOT, record.zxs_data.common_data)?;
        let retained = open(&management, &layout::DELETED_DATA_ROOT, record.zxs_data.deleted_data)?;
        let result = Self { common, deleted, management, current, retained, record };
        result.verify(parent)?;
        Ok(result)
    }

    pub(super) fn verify(&self, parent: &DiskRoot) -> Result<()> {
        for (name, held, expected) in [
            (&*layout::COMMON_DATA_ROOT, &self.common, self.record.common_data),
            (&*layout::DELETED_DATA_ROOT, &self.deleted, self.record.deleted_data),
            (&*layout::ZXS_DATA_ROOT, &self.management, self.record.zxs_data.root),
        ] {
            require(parent, name, held, expected)?;
        }
        for (name, held, expected) in [
            (&*layout::COMMON_DATA_ROOT, &self.current, self.record.zxs_data.common_data),
            (&*layout::DELETED_DATA_ROOT, &self.retained, self.record.zxs_data.deleted_data),
        ] {
            require(&self.management, name, held, expected)?;
        }
        Ok(())
    }

    pub(super) fn record(&self) -> ContainerAreas {
        self.record
    }

    pub(super) fn common(&self) -> &DiskRoot {
        &self.common
    }

    pub(super) fn current_zxs(&self) -> &Arc<DiskRoot> {
        &self.current
    }
}

fn identity(entity: ContainerEntity) -> Result<FileIdentity> {
    if entity.inode() == 0 {
        return Err(Failure::ContainerIdentity.into());
    }
    Ok(FileIdentity { device: entity.device(), inode: entity.inode() })
}

fn open(root: &DiskRoot, name: &EntryName, expected: ContainerEntity) -> Result<Arc<DiskRoot>> {
    let expected = identity(expected)?;
    let observed = root.private_entry_matching(name, expected)
        .map_err(|error| match error {
            changed @ (crate::disk::DirectoryIoError::IdentityChanged(_)
            | crate::disk::DirectoryIoError::WrongRoot) => {
                Failure::ContainerArea(std::io::Error::other(changed)).into()
            }
            other => crate::engine::fresh_layout::directory_error(other),
        })?;
    Ok(Arc::new(observed))
}

fn require(
    root: &DiskRoot, name: &EntryName, held: &DiskRoot, expected: ContainerEntity,
) -> Result<()> {
    let observed = open(root, name, expected)?;
    if held.directory_identity(held.path())? != observed.directory_identity(observed.path())? {
        return Err(Failure::ContainerIdentity.into());
    }
    Ok(())
}
