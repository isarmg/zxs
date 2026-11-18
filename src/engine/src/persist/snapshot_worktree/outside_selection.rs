//! One descriptive FILE selection inside the original Worktree mapping
//! decision. Decoding even a perfectly formed copy conveys no FILE read,
//! path ownership, inclusion history, or authorization to remove a mapping.
use data_serialization::{Deserialize, Deserializer, Serialize};
use name_policy::{Name, SlotId};
use storage_contract::file_container::{
    ContainerEntity, NamespaceDirectoryParent, site::{FileSiteWitness, SitePurpose},
};
use storage_contract::file_draft::{DraftBase, DraftLocation};
use storage_contract::working_tree::{DirectoryId, LocalEntityIdentity, RootId};
use storage_contract::{
    BranchId, ContentHash, DiskId, FileNodeId, Generation, HistoryTreeId, OperationId, PoolId,
    Sequence, VersionStorageId,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct OutsideSelectionFields {
    pub(crate) target: DraftLocation,
    pub(crate) selection: DraftBase,
    pub(crate) logical_name: Name,
    pub(crate) history: HistoryTreeId,
    pub(crate) birth: OperationId,
    pub(crate) namespace_branch: BranchId,
    pub(crate) original_parent: NamespaceDirectoryParent,
    pub(crate) site: FileSiteWitness,
    pub(crate) pool: PoolId,
    pub(crate) owner: DiskId,
    pub(crate) slot: SlotId,
    pub(crate) owner_generation: Generation,
    pub(crate) root: RootId,
    pub(crate) root_entity: LocalEntityIdentity,
    pub(crate) container_preparation: DirectoryId,
    pub(crate) container: ContainerEntity,
    pub(crate) body_parent: LocalEntityIdentity,
    pub(crate) body: LocalEntityIdentity,
    pub(crate) storage: VersionStorageId,
    #[serialization(with = "storage_contract::u64_string")]
    pub(crate) size: u64,
    pub(crate) hash: ContentHash,
    pub(crate) version_sequence: Sequence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serialization(transparent)]
pub(crate) struct OutsideSelectionWitness(OutsideSelectionFields);

impl OutsideSelectionWitness {
    pub(crate) fn fields(&self) -> &OutsideSelectionFields { &self.0 }

    /// Shape and internal identity consistency only. Native FILE admission
    /// and the original Worktree mapping are independent caller duties.
    pub(crate) fn checked(fields: OutsideSelectionFields) -> Option<Self> {
        let generation = fields.site.generation.get();
        let valid_site = match fields.site.purpose {
            SitePurpose::Created => generation == 1,
            SitePurpose::Restored => generation >= 3 && generation % 2 == 1,
            SitePurpose::Retained => generation >= 2 && generation % 2 == 0,
        };
        let valid_node = match fields.selection.node_id() {
            FileNodeId::Version(id) => id == fields.selection.version_id(),
            FileNodeId::Anchor(_) => true,
            FileNodeId::Metadata(_) | FileNodeId::Merge(_) => false,
        };
        let original_parent = match fields.original_parent {
            NamespaceDirectoryParent::Root => fields.target.path.parent().is_none(),
            NamespaceDirectoryParent::Directory(id) => fields.target.path.parent().is_some() && id != fields.target.entry,
        };
        let container = LocalEntityIdentity::new(fields.container.device(), fields.container.inode());
        let mut entities = std::collections::BTreeSet::new();
        if ![fields.root_entity, container, fields.body_parent, fields.body].into_iter().all(|entity|
            entity.device() == container.device() && entity.inode() != 0 && entities.insert(entity))
            || !valid_node || !original_parent || !valid_site
            || fields.owner_generation.get() == 0 || fields.version_sequence.get() == 0
            || !crate::container_storage::namespace_host::ordinary_path(&fields.target.path)
            || fields.storage != VersionStorageId::from_version(fields.selection.version_id())
        {
            return None;
        }
        Some(Self(fields))
    }
}

impl<'de> Deserialize<'de> for OutsideSelectionWitness {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let fields = OutsideSelectionFields::deserialize(deserializer)?;
        Self::checked(fields).ok_or_else(|| data_serialization::de::Error::custom(
            "inconsistent outside FILE mapping identity",
        ))
    }
}
