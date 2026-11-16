//! A container's logical owner and observed local replicas. Addresses and open
//! descriptors are deliberately absent. These records require native admission.
use crate::{BranchId, DiskId, EntryId, HistoryTreeId, OperationId, RepositoryId};
use crate::file_draft::FileWorkingState;
use crate::working_tree::{DirectoryId, RootId};
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

/// Fixed-width wire evidence makes an unobserved preparation's metadata bound
/// independent of the eventual device/inode numbers. Zero is a budget shape,
/// never an admitted entity; native snapshot admission must reject it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ContainerEntity {
    device: u64,
    inode: u64,
}
impl ContainerEntity {
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
impl Serialize for ContainerEntity {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{:016x}{:016x}", self.device, self.inode))
    }
}
impl<'de> Deserialize<'de> for ContainerEntity {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.len() != 32
            || !value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(data_serialization::de::Error::custom("invalid container entity"));
        }
        let device = u64::from_str_radix(&value[..16], 16)
            .map_err(data_serialization::de::Error::custom)?;
        let inode = u64::from_str_radix(&value[16..], 16)
            .map_err(data_serialization::de::Error::custom)?;
        Ok(Self { device, inode })
    }
}

/// One source slice, including the management partitions belonging to its
/// current and retained objects. These are physical observations, not permission
/// to move an object between lifecycles.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ContainerManagementAreas {
    pub root: ContainerEntity,
    pub common_data: ContainerEntity,
    pub deleted_data: ContainerEntity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ContainerAreas {
    pub common_data: ContainerEntity,
    pub deleted_data: ContainerEntity,
    pub zxs_data: ContainerManagementAreas,
}
impl ContainerAreas {
    /// Fixed-width capacity shape only. Native admission rejects every zero inode.
    pub const fn budget() -> Self {
        let empty = ContainerEntity::new(0, 0);
        Self {
            common_data: empty,
            deleted_data: empty,
            zxs_data: ContainerManagementAreas {
                root: empty, common_data: empty, deleted_data: empty,
            },
        }
    }

    pub fn entities(self) -> [ContainerEntity; 5] {
        [self.common_data, self.deleted_data, self.zxs_data.root,
            self.zxs_data.common_data, self.zxs_data.deleted_data]
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ContainerPrefix {
    pub source_disk: DiskId,
    pub preparation: DirectoryId,
    pub entity: ContainerEntity,
    pub areas: ContainerAreas,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ContainerReplica {
    pub root_id: RootId,
    /// Immediate source first. Shared prefixes repeat the same published identity.
    pub prefixes: Vec<ContainerPrefix>,
    /// The exclusive preparation entry, retained as recovery causality even after install.
    pub preparation: DirectoryId,
    pub entity: ContainerEntity,
}

/// Logical parent identity is independent of any physical occurrence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", content = "entry", rename_all = "snake_case",
    deny_unknown_fields)]
pub enum NamespaceDirectoryParent {
    Root,
    Directory(EntryId),
}

/// An occurrence on one target disk. The ordered full source registrations
/// distinguish B/A and C/B/A. Parent identity is physical evidence, not a path.
/// Preparation doubles as this occurrence's stable DirectoryId after rename.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct NamespaceDirectoryReplica {
    pub root_id: RootId,
    pub source_chain: Vec<crate::working_tree::DiskRegistrationIdentity>,
    pub parent_entity: ContainerEntity,
    pub preparation: DirectoryId,
    pub entity: ContainerEntity,
    pub lifecycle: crate::working_tree::BindingLifecycle,
    pub created_by: OperationId,
}

/// One logical EntryId, with independently decided physical occurrences. The
/// logical identity is not duplicated when FILE-triggered B/C parents are added.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct NamespaceDirectoryRecord {
    pub repository: RepositoryId,
    pub branch: BranchId,
    pub entry: EntryId,
    pub parent: NamespaceDirectoryParent,
    pub name: name_policy::LookupName,
    pub owner_disk: DiskId,
    pub created_by: OperationId,
    pub replicas: BTreeMap<DiskId, NamespaceDirectoryReplica>,
}

/// The namespace occurrence selecting the ordinary container's physical name.
/// It is checked against the current main namespace and its published root
/// registration. A path string, global FILE selection or latest route is not it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ContainerNamespace {
    pub repository: RepositoryId,
    pub branch: BranchId,
    pub entry: EntryId,
    pub parent: NamespaceDirectoryParent,
    pub name: name_policy::LookupName,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct FileContainerRecord {
    pub history_tree_id: HistoryTreeId,
    pub owner_disk: DiskId,
    pub created_by: OperationId,
    pub namespace: ContainerNamespace,
    pub working: FileWorkingState,
    pub replicas: BTreeMap<DiskId, ContainerReplica>,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/file_container_tests.rs"]
mod tests;
