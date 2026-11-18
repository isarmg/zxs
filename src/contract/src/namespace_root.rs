//! Ordinary registered storage: durable identities, local root geometry and
//! plain directory ancestry. Decoding these facts alone grants no disk I/O.
use crate::{Generation, Id, OperationId};
use data_serialization::{Deserialize, Deserializer, Serialize};
use name_policy::LookupName;

pub type RootId = Id<crate::kind::Root>;
pub type DirectoryId = Id<crate::kind::Directory>;

/// Physical storage layout, independent of metadata-page and disk-seal formats.
/// There is deliberately no default or old-format fallback.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum StorageLayout {
    UnifiedWorkingTreeV1,
}
impl<'de> Deserialize<'de> for StorageLayout {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "unified_working_tree_v1" => Ok(Self::UnifiedWorkingTreeV1),
            _ => Err(data_serialization::de::Error::custom("unsupported storage layout")),
        }
    }
}

/// The exact pool, disk, slot and binding generation selected by the registry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiskRegistrationIdentity {
    pub pool_id: String,
    pub disk_id: String,
    pub slot_id: String,
    #[serialization(with = "crate::u64_string")]
    pub binding_generation: u64,
}

/// Durable local evidence, never a path, logical ID, FD or permission to write.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct LocalEntityIdentity {
    #[serialization(with = "crate::u64_string")]
    device: u64,
    #[serialization(with = "crate::u64_string")]
    inode: u64,
}
impl LocalEntityIdentity {
    pub const fn new(device: u64, inode: u64) -> Self {
        Self { device, inode }
    }
    pub const fn device(self) -> u64 {
        self.device
    }
    pub const fn inode(self) -> u64 {
        self.inode
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct LocalDirectoryBinding {
    pub directory_id: DirectoryId,
    pub entity: LocalEntityIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ZxsLifecycleBindings {
    pub common_data: LocalDirectoryBinding,
    pub deleted_data: LocalDirectoryBinding,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ThreeRootBinding {
    pub common_data: LocalDirectoryBinding,
    pub deleted_data: LocalDirectoryBinding,
    pub zxs_data: LocalDirectoryBinding,
    pub zxs_lifecycles: ZxsLifecycleBindings,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct RootBinding {
    pub layout: StorageLayout,
    pub root_id: RootId,
    pub registration: DiskRegistrationIdentity,
    /// Immediate source first, original primary last. Primary roots use [].
    pub source_chain: Vec<DiskRegistrationIdentity>,
    pub root_entity: LocalEntityIdentity,
    pub roots: ThreeRootBinding,
    pub operation_id: OperationId,
    pub generation: Generation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BindingLifecycle {
    Current,
    Retained { operation_id: OperationId },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryBinding {
    pub directory_id: DirectoryId,
    pub root_id: RootId,
    /// May name a root's common/deleted anchor or another directory, never FILE.
    pub parent_directory_id: DirectoryId,
    /// Exact original spelling, including admitted normalization differences.
    pub name: LookupName,
    pub entity: LocalEntityIdentity,
    pub lifecycle: BindingLifecycle,
    pub operation_id: OperationId,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/namespace_root_tests.rs"]
mod tests;
