//! Durable mixed-directory identities. Decoding is not authority admission.
//!
//! These records are a standalone prerequisite, not fields of current state.
//! Persisting or activating them requires a separate authenticated format boundary;
//! FILE metadata never receives these locations.
use crate::{ContentHash, FileId, Generation, Id, OperationId};
use data_serialization::{Deserialize, Deserializer, Serialize};
use name_policy::LookupName;

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

pub mod kind {
    use crate::IdKind;

    pub enum Root {}
    impl IdKind for Root {
        const PREFIX: &'static str = "root";
    }
    pub enum Directory {}
    impl IdKind for Directory {
        const PREFIX: &'static str = "directory";
    }
    pub enum Replica {}
    impl IdKind for Replica {
        const PREFIX: &'static str = "replica";
    }
    pub enum SnapshotRepository {}
    impl IdKind for SnapshotRepository {
        const PREFIX: &'static str = "snapshot_repo";
    }
    pub enum SnapshotNode {}
    impl IdKind for SnapshotNode {
        const PREFIX: &'static str = "snapshot_node";
    }
}

pub type RootId = Id<kind::Root>;
pub type DirectoryId = Id<kind::Directory>;
pub type ReplicaId = Id<kind::Replica>;
pub type SnapshotRepositoryId = Id<kind::SnapshotRepository>;
/// Immutable full snapshot identity, independent of the retired directory Git trees.
pub type SnapshotNodeId = Id<kind::SnapshotNode>;

/// The existing exact registration tuple, shared with the engine registry.
/// Its historical wire spelling is unchanged by moving the type to contract.
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
pub struct ThreeRootBinding {
    pub common_data: LocalDirectoryBinding,
    pub deleted_data: LocalDirectoryBinding,
    pub zxs_data: LocalDirectoryBinding,
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

/// Object-material commitments are not file-body hashes or authorization.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(transparent)]
pub struct MaterialCommitment(ContentHash);
impl MaterialCommitment {
    pub fn from_digest(digest: blake3::Hash) -> Self {
        Self(ContentHash::from_digest(digest))
    }
    pub const fn hash(self) -> ContentHash {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ContainerBinding {
    pub file_id: FileId,
    pub replica_id: ReplicaId,
    pub root_id: RootId,
    pub parent_directory_id: DirectoryId,
    pub name: LookupName,
    pub lifecycle: BindingLifecycle,
    /// Binds existing FILE authority; never replaces its graph or selections.
    pub file_authority: MaterialCommitment,
    pub operation_id: OperationId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SnapshotRepositoryBinding {
    pub repository_id: SnapshotRepositoryId,
    pub replica_id: ReplicaId,
    pub root_id: RootId,
    /// Original host identity survives retaining only the management repository.
    /// For retained repositories this is historical provenance, not a live
    /// parent reference or authority to open a host that may since be deleted.
    pub host_directory_id: DirectoryId,
    pub location: SnapshotRepositoryLocation,
    pub material: MaterialCommitment,
    pub operation_id: OperationId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SnapshotRepositoryLocation {
    /// Fixed management entry below the original host, never client-selectable.
    Attached,
    Retained {
        parent_directory_id: DirectoryId,
        name: LookupName,
        operation_id: OperationId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serialization(tag = "kind", content = "id", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReplicaObject {
    Container(FileId),
    SnapshotRepository(SnapshotRepositoryId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum ReplicaPurpose {
    Current,
    Backup,
    Retained,
}
impl<'de> Deserialize<'de> for ReplicaPurpose {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "current" => Ok(Self::Current),
            "backup" => Ok(Self::Backup),
            "retained" => Ok(Self::Retained),
            _ => Err(data_serialization::de::Error::custom("unsupported replica purpose")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ReplicaBinding {
    pub replica_id: ReplicaId,
    pub object: ReplicaObject,
    pub root_id: RootId,
    pub registration: DiskRegistrationIdentity,
    pub source_chain: Vec<DiskRegistrationIdentity>,
    pub entity: LocalEntityIdentity,
    pub purpose: ReplicaPurpose,
    pub operation_id: OperationId,
}

/// Standalone structural catalog, not current global state or an authority marker.
/// Its own wire record requires all fields, including empty object tables.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct WorkingTreeCatalog {
    pub storage_layout: StorageLayout,
    /// Strict RootId order. Duplicate or out-of-order rows are rejected.
    pub roots: Vec<RootBinding>,
    /// Strict DirectoryId order, including ordinary directories without history.
    pub directories: Vec<DirectoryBinding>,
    /// Object placements use strict ReplicaId order, not FileId order.
    pub containers: Vec<ContainerBinding>,
    /// Strict ReplicaId order allows one logical repository on multiple disks.
    pub snapshot_repositories: Vec<SnapshotRepositoryBinding>,
    /// Strict ReplicaId order. Every row must close exactly one object binding.
    pub replicas: Vec<ReplicaBinding>,
}

pub const MAX_DIRECTORY_TARGET_BYTES: usize = 4096;
pub const MAX_DIRECTORY_TARGET_COMPONENTS: usize = 64;

/// A bounded exact relative spelling. An empty path is represented separately.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serialization(transparent)]
pub struct DirectoryRelativePath(Vec<LookupName>);

#[derive(Debug, error_definition::Error)]
pub enum DirectoryTargetError {
    #[error("directory target exceeds its path budget")]
    Budget,
    #[error("directory target must be a nonempty relative path")]
    Empty,
    #[error("directory target contains an invalid component")]
    Name(#[source] name_policy::Error),
}
impl DirectoryRelativePath {
    pub fn parse(path: &str) -> Result<Self, DirectoryTargetError> {
        if path.is_empty() {
            return Err(DirectoryTargetError::Empty);
        }
        if path.len() > MAX_DIRECTORY_TARGET_BYTES {
            return Err(DirectoryTargetError::Budget);
        }
        let mut names = Vec::new();
        for component in path.split('/') {
            if names.len() == MAX_DIRECTORY_TARGET_COMPONENTS {
                return Err(DirectoryTargetError::Budget);
            }
            names.push(LookupName::parse(component).map_err(DirectoryTargetError::Name)?);
        }
        Ok(Self(names))
    }
    pub fn components(&self) -> &[LookupName] {
        &self.0
    }
}
impl<'de> Deserialize<'de> for DirectoryRelativePath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let names = Vec::<LookupName>::deserialize(deserializer)?;
        if names.is_empty() {
            return Err(data_serialization::de::Error::custom(DirectoryTargetError::Empty));
        }
        if names.len() > MAX_DIRECTORY_TARGET_COMPONENTS {
            return Err(data_serialization::de::Error::custom(DirectoryTargetError::Budget));
        }
        let bytes = names.iter().map(|name| name.as_str().len()).sum::<usize>();
        if bytes + names.len() - 1 > MAX_DIRECTORY_TARGET_BYTES {
            return Err(data_serialization::de::Error::custom(DirectoryTargetError::Budget));
        }
        Ok(Self(names))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", content = "path", rename_all = "snake_case", deny_unknown_fields)]
pub enum DirectoryTargetPath {
    Root,
    Relative(DirectoryRelativePath),
}

/// A target need not already have a DirectoryId. The deciding operation assigns
/// every missing ancestor identity using one held and revalidated path walk.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryTargetRequest {
    pub root_id: RootId,
    pub path: DirectoryTargetPath,
    pub expected_root_generation: Generation,
    pub operation_id: OperationId,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/working_tree_tests.rs"]
mod tests;
