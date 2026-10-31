use crate::SharedRecords;
use data_serialization::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

macro_rules! strict_string_deserialize {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: data_serialization::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct StringVisitor;
                impl data_serialization::de::Visitor<'_> for StringVisitor {
                    type Value = $name;
                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str(concat!(stringify!($name), " string"))
                    }
                    fn visit_str<E: data_serialization::de::Error>(self, token: &str) -> Result<Self::Value, E> {
                        match token {
                            $($wire => Ok($name::$variant),)+
                            _ => Err(E::unknown_variant(token, &[$($wire),+])),
                        }
                    }
                }
                deserializer.deserialize_str(StringVisitor)
            }
        }
    };
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serialization(rename_all = "snake_case")]
pub enum Mode {
    Production,
    Development,
}
strict_string_deserialize!(Mode { Production => "production", Development => "development" });

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiskConfig {
    pub slot_id: String,
    pub disk_id: String,
    pub root: PathBuf,
    pub level: u32,
    pub upstream_slot_id: Option<String>,
    pub metadata_member: bool,
    #[serialization(with = "crate::u64_string")]
    pub binding_generation: u64,
    /// Optional explicit admission budget, in addition to actual free space.
    #[serialization(default, with = "crate::optional_u64_string")]
    pub capacity_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Config {
    pub pool_id: String,
    pub mode: Mode,
    pub disks: Vec<DiskConfig>,
    pub checksum_chunk_bytes: u32,
    #[serialization(with = "crate::u64_string")]
    pub max_file_bytes: u64,
    #[serialization(with = "crate::u64_string")]
    pub max_workspace_bytes: u64,
    pub max_workspaces: usize,
    /// Durable records, including committed and aborted recovery evidence.
    #[serialization(default = "default_max_retained_workspaces")]
    pub max_retained_workspaces: usize,
    #[serialization(default = "default_max_retained_workspaces_per_principal")]
    pub max_retained_workspaces_per_principal: usize,
    /// Logical manifest bytes, charged once per record, with lifecycle headroom.
    #[serialization(
        default = "default_max_retained_workspace_metadata_bytes",
        with = "crate::u64_string"
    )]
    pub max_retained_workspace_metadata_bytes: u64,
    #[serialization(
        default = "default_max_retained_workspace_metadata_bytes_per_principal",
        with = "crate::u64_string"
    )]
    pub max_retained_workspace_metadata_bytes_per_principal: u64,
    pub max_entries: usize,
    #[serialization(with = "crate::u64_string")]
    pub metadata_reserve_bytes: u64,
    #[serialization(with = "crate::u64_string")]
    pub workspace_ttl_seconds: u64,
}
fn default_max_retained_workspaces() -> usize {
    4096
}
fn default_max_retained_workspaces_per_principal() -> usize {
    1024
}
fn default_max_retained_workspace_metadata_bytes() -> u64 {
    128 << 20
}
fn default_max_retained_workspace_metadata_bytes_per_principal() -> u64 {
    64 << 20
}
impl Config {
    /// Build a development draft with one primary and optional ordered backup tiers.
    /// Validate the draft before use; an empty iterator is not a valid pool.
    pub fn development(disks: impl IntoIterator<Item = PathBuf>) -> Self {
        fn slot(index: usize) -> String {
            match index {
                0 => "P01".into(),
                1 => "B01".into(),
                2 => "C01".into(),
                _ => format!("D{:02}", index + 1),
            }
        }
        Self {
            pool_id: crate::new_id("pool"),
            mode: Mode::Development,
            disks: disks
                .into_iter()
                .enumerate()
                .map(|(i, root)| DiskConfig {
                    slot_id: slot(i),
                    disk_id: crate::new_id("disk"),
                    root,
                    level: i as u32 + 1,
                    upstream_slot_id: i.checked_sub(1).map(slot),
                    metadata_member: true,
                    binding_generation: 1,
                    capacity_bytes: None,
                })
                .collect(),
            checksum_chunk_bytes: 262144,
            max_file_bytes: 1 << 40,
            max_workspace_bytes: 1 << 40,
            max_workspaces: 128,
            max_retained_workspaces: default_max_retained_workspaces(),
            max_retained_workspaces_per_principal: default_max_retained_workspaces_per_principal(),
            max_retained_workspace_metadata_bytes: default_max_retained_workspace_metadata_bytes(),
            max_retained_workspace_metadata_bytes_per_principal:
                default_max_retained_workspace_metadata_bytes_per_principal(),
            max_entries: 100_000,
            metadata_reserve_bytes: 64 << 20,
            workspace_ttl_seconds: 86_400,
        }
    }

    /// Recovery/admission majority of the configured membership, never the
    /// currently reachable disks. Normal publication still requires all members.
    /// Empty memberships are invalid and cannot obtain this minimum quorum.
    pub fn metadata_quorum(&self) -> usize {
        self.disks
            .iter()
            .filter(|disk| disk.metadata_member)
            .count()
            / 2
            + 1
    }
}

macro_rules! string_state {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
        #[serialization(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire),+ }
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: data_serialization::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct StateVisitor;
                impl data_serialization::de::Visitor<'_> for StateVisitor {
                    type Value = $name;
                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str(concat!(stringify!($name), " string"))
                    }
                    fn visit_str<E: data_serialization::de::Error>(self, token: &str) -> Result<Self::Value, E> {
                        match token {
                            $($wire => Ok($name::$variant),)+
                            _ => Err(E::unknown_variant(token, &[$($wire),+])),
                        }
                    }
                }
                deserializer.deserialize_str(StateVisitor)
            }
        }
    };
}

string_state!(BranchKind { File => "file", Full => "full" });
string_state!(FileMetadataKind { Tags => "tags", Merge => "merge" });
string_state!(HistoryState { Active => "active", Archived => "archived" });
string_state!(BranchState {
    Active => "active", Archived => "archived", Migrated => "migrated", Merged => "merged"
});
string_state!(WorkspaceState {
    Open => "open", Sealed => "sealed", Committed => "committed", Aborted => "aborted"
});
string_state!(WorkspacePurpose { Normal => "normal", HistoricalBranch => "historical_branch" });
string_state!(PlanState {
    Ready => "ready", Cancelled => "cancelled", Committed => "committed", Completed => "completed"
});
string_state!(NamespaceOperationKind {
    Mkdir => "mkdir", Rename => "rename", Delete => "delete", DeleteTree => "delete_tree"
});

impl std::str::FromStr for NamespaceOperationKind {
    type Err = crate::Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "mkdir" => Ok(Self::Mkdir),
            "rename" => Ok(Self::Rename),
            "delete" => Ok(Self::Delete),
            "delete_tree" => Ok(Self::DeleteTree),
            _ => Err(crate::Error::new(
                crate::ErrorCode::OperationUnsupported,
                "namespace operation is not supported",
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct Repository {
    pub repository_id: String,
    pub name: String,
    pub history_tree_id: String,
    pub main_branch_id: String,
    pub primary_slot_id: String,
    pub target_copies: u32,
    pub name_policy: String,
    pub unicode_version: String,
    pub smb_enabled: bool,
    /// Persistent SMB binding revision, independent of ordinary history writes.
    /// Every explicit rebind/disable invalidates sessions holding an older value.
    #[serialization(with = "crate::u64_string")]
    pub smb_binding_generation: u64,
    /// A controlled disk-root namespace, created lazily by the file manager.
    #[serialization(default, skip_serializing_if = "is_false")]
    pub file_manager_root: bool,
}
fn is_zero(value: &u64) -> bool {
    *value == 0
}
fn is_false(value: &bool) -> bool {
    !*value
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct HistoryTree {
    pub history_tree_id: String,
    pub repository_id: String,
    pub root_commit_id: String,
    pub main_branch_id: Option<String>,
    #[serialization(with = "crate::u64_string")]
    pub generation: u64,
    pub state: HistoryState,
    pub provenance: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct Branch {
    pub branch_id: String,
    pub history_tree_id: String,
    pub name: String,
    pub head_commit_id: String,
    pub fork_commit_id: String,
    pub branch_kind: BranchKind,
    /// File branches contain only this stable file identity and its parents.
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<String>,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    pub file_node_id: Option<String>,
    /// Unwritten file identities and frozen parent versions. Reads inherit
    /// these versions until a first write creates branch-owned content.
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending_file_versions: BTreeMap<String, String>,
    #[serialization(with = "crate::u64_string")]
    pub generation: u64,
    pub state: BranchState,
}
/// A directory view composes shared file branches at frozen content versions.
/// It owns neither private file branches nor payloads. Edits advance its
/// immutable subtree head without changing the named branch identity.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryBranch {
    pub directory_branch_id: String,
    pub repository_id: String,
    pub directory_id: String,
    pub path: String,
    pub name: String,
    pub parent_branch_id: Option<String>,
    pub head_tree_id: String,
    pub commit_id: String,
    pub entries: Vec<Entry>,
    pub file_branches: BTreeMap<String, String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_contexts: BTreeMap<String, FileContext>,
    /// Frozen write intent: inherited entries fork publicly only on real save.
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inherited_files: BTreeMap<String, String>,
    /// Exact child composition references; never follow a mutable HEAD.
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub directory_refs: BTreeMap<String, DirectoryReference>,
    pub materialized: bool,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryTree {
    pub tree_id: String,
    pub directory_branch_id: String,
    pub parent_tree_id: Option<String>,
    pub commit_id: String,
    pub path: String,
    pub entries: Vec<Entry>,
    pub file_branches: BTreeMap<String, String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_contexts: BTreeMap<String, FileContext>,
    /// Frozen write intent: inherited entries fork publicly only on real save.
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inherited_files: BTreeMap<String, String>,
    /// Exact child composition references; never follow a mutable HEAD.
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub directory_refs: BTreeMap<String, DirectoryReference>,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
}
/// An immutable namespace event in a public file's history. The body remains
/// the referenced real FileVersion; a rename creates no payload or fake FULL.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct FilePathNode {
    pub node_id: String,
    pub history_tree_id: String,
    pub file_id: String,
    pub version_id: String,
    pub parent_node_ids: Vec<String>,
    pub branch_id: String,
    pub path: String,
    pub previous_path: String,
    pub operation_id: crate::OperationId,
    pub author: String,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    #[serialization(with = "crate::u64_string")]
    pub created_at: u64,
}

/// An immutable metadata revision. Its body is an existing verified FULL;
/// metadata edits never manufacture a content version or alter its proof.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct FileMetadataNode {
    pub node_id: String,
    pub history_tree_id: String,
    pub file_id: String,
    pub version_id: String,
    pub parent_node_ids: Vec<String>,
    pub branch_id: String,
    pub kind: FileMetadataKind,
    pub tags: BTreeSet<String>,
    pub operation_id: crate::OperationId,
    pub author: String,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    #[serialization(with = "crate::u64_string")]
    pub created_at: u64,
}

/// Merge provenance is separate from immutable content/path parent proofs.
/// Reusing an existing head records every input even when an ancestry edge
/// must be omitted to keep the displayed history acyclic.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct FileBranchMerge {
    pub merge_id: String,
    pub history_tree_id: String,
    pub file_id: String,
    pub input_branch_ids: Vec<String>,
    pub input_node_ids: Vec<String>,
    pub chosen_node_id: String,
    pub result_node_id: String,
    pub surviving_branch_id: String,
    pub reused_node: bool,
    pub parent_node_ids: Vec<String>,
    pub operation_id: crate::OperationId,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct FileMergeInput {
    pub branch_id: String,
    pub expected_node_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct FileTagTarget {
    pub branch_id: String,
    pub source_node_id: String,
    pub expected_node_id: String,
}

/// A logical path's write target; several aliases of one identity may expose
/// different immutable versions and independently inherited write intent.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
pub struct FileContext {
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serialization(default, skip_serializing_if = "is_zero", with = "crate::u64_string")]
    pub selection_generation: u64,
    pub branch_id: String,
    pub inherited_from: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryReference {
    pub directory_branch_id: String,
    pub tree_id: String,
}

string_state!(DirectoryBatchKind { Move => "move", Copy => "copy", Delete => "delete", Rename => "rename" });

/// One atomic namespace request, with an explicit immutable scope per item.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryNamespaceBatch {
    pub kind: DirectoryBatchKind,
    pub items: Vec<DirectoryNamespaceItem>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryNamespaceItem {
    pub source: DirectoryNamespaceSource,
    pub target: Option<DirectoryNamespaceTarget>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryNamespaceSource {
    pub directory_branch_id: String,
    pub expected_tree_id: String,
    pub path: String,
    pub entry_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryNamespaceTarget {
    pub directory_branch_id: String,
    pub expected_tree_id: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub entry_id: String,
    pub path: name_policy::PortablePath,
    pub kind: NamespaceEntryKind,
    pub file_id: String,
    pub version_id: String,
    pub size: u64,
}
impl Entry {
    pub fn path(&self) -> &str {
        self.path.as_str()
    }
    pub fn name(&self) -> &str {
        self.path.file_name()
    }
    pub fn lookup_key(&self) -> &str {
        self.path.key()
    }
    pub fn location(&self) -> &name_policy::PortablePath {
        &self.path
    }
    pub fn relocate(&mut self, location: name_policy::PortablePath) {
        self.path = location;
    }
}
impl Serialize for Entry {
    fn serialize<S: data_serialization::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        use data_serialization::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Entry", 8)?;
        state.serialize_field("entry_id", &self.entry_id)?;
        state.serialize_field("path", self.path())?;
        state.serialize_field("name", self.name())?;
        state.serialize_field("lookup_key", self.lookup_key())?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("file_id", &self.file_id)?;
        state.serialize_field("version_id", &self.version_id)?;
        state.serialize_field("size", &self.size.to_string())?;
        state.end()
    }
}
impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serialization(deny_unknown_fields)]
        struct WireEntry {
            entry_id: String,
            path: String,
            name: String,
            lookup_key: String,
            kind: NamespaceEntryKind,
            file_id: String,
            version_id: String,
            #[serialization(with = "crate::u64_string")]
            size: u64,
        }
        let raw = WireEntry::deserialize(deserializer)?;
        let location = name_policy::PortablePath::parse(&raw.path)
            .map_err(data_serialization::de::Error::custom)?;
        if raw.name != location.file_name() || raw.lookup_key != location.key() {
            return Err(data_serialization::de::Error::custom(
                "entry name or lookup key differs from path",
            ));
        }
        Ok(Self {
            entry_id: raw.entry_id,
            path: location,
            kind: raw.kind,
            file_id: raw.file_id,
            version_id: raw.version_id,
            size: raw.size,
        })
    }
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serialization(rename_all = "snake_case")]
pub enum NamespaceEntryKind {
    File,
    Directory,
}
impl<'de> Deserialize<'de> for NamespaceEntryKind {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        // Stored kinds are strings; a derived enum also accepts {"file": null}.
        struct KindVisitor;
        impl data_serialization::de::Visitor<'_> for KindVisitor {
            type Value = NamespaceEntryKind;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("file or directory as a string")
            }
            fn visit_str<E: data_serialization::de::Error>(
                self,
                token: &str,
            ) -> Result<Self::Value, E> {
                match token {
                    "file" => Ok(NamespaceEntryKind::File),
                    "directory" => Ok(NamespaceEntryKind::Directory),
                    _ => Err(E::unknown_variant(token, &["file", "directory"])),
                }
            }
        }
        deserializer.deserialize_str(KindVisitor)
    }
}
/// A logical deletion has no content identity or payload-retention dependency.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
pub struct DeletedPath {
    pub path: String,
    pub kind: NamespaceEntryKind,
}
/// Ordered removals performed by this commit, independent of later path reuse.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
pub struct NamespaceDeletions {
    #[serialization(with = "crate::u64_string")]
    pub timestamp: u64,
    pub paths: Vec<DeletedPath>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct Commit {
    pub commit_id: String,
    pub history_tree_id: String,
    pub kind: String,
    pub parent_commit_ids: Vec<String>,
    pub tree_id: String,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    pub operation_id: crate::OperationId,
    pub author: String,
    pub message: String,
    pub entries: Vec<Entry>,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    pub namespace_deletions: Option<NamespaceDeletions>,
    /// Snapshot-local reservations preserve inherited parent views even after
    /// the live branch later gains its own content versions.
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending_file_versions: BTreeMap<String, String>,
    pub provenance: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct FileVersion {
    pub history_tree_id: String,
    pub version_id: String,
    pub file_id: String,
    pub creation_primary_slot_id: String,
    #[serialization(with = "crate::u64_string")]
    pub logical_size: u64,
    #[serialization(with = "crate::u64_string")]
    pub version_sequence: u64,
    pub content_hash: crate::ContentHash,
    pub checksum_chunk_bytes: u32,
    pub checksum_manifest_hash: String,
    pub creation_name: String,
    pub storage_name: String,
    pub created_at: String,
    pub source_version_ids: Vec<String>,
    pub created_as_full_checkpoint: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct Placement {
    pub version_id: String,
    pub primary_slot_id: String,
    pub target_copies: u32,
    pub route_slot_ids: Vec<String>,
    #[serialization(with = "crate::u64_string")]
    pub topology_generation: u64,
    #[serialization(with = "crate::u64_string_map")]
    pub binding_generations: BTreeMap<String, u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct MergeLink {
    pub merge_link_id: String,
    pub history_tree_id: String,
    pub source_branch_id: String,
    pub source_commit_id: String,
    pub target_branch_id: String,
    pub target_commit_id: String,
    pub author: String,
    pub operation_id: crate::OperationId,
}
/// Audit-only edge across a split boundary. External identifiers are provenance,
/// never traversed as live commits or branches of this history.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct HistoryBoundary {
    pub boundary_id: String,
    pub history_tree_id: String,
    pub local_commit_id: String,
    pub local_branch_id: Option<String>,
    pub relation: String,
    pub external_history_tree_id: String,
    pub external_commit_id: String,
    pub source_merge_link_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct StagedFile {
    pub item_id: String,
    pub path: name_policy::PortablePath,
    pub source_path: PathBuf,
    #[serialization(with = "crate::u64_string")]
    pub size: u64,
    pub content_hash: crate::ContentHash,
    #[serialization(with = "crate::u64_string")]
    pub dirty_generation: u64,
    pub base_version_id: Option<String>,
}
#[derive(Clone, Debug)]
pub enum NamespaceOperation {
    Mkdir {
        path: name_policy::PortablePath,
    },
    Delete {
        path: name_policy::LookupPath,
    },
    DeleteTree {
        path: name_policy::LookupPath,
    },
    Rename {
        source: name_policy::LookupPath,
        destination: name_policy::PortablePath,
    },
}
impl NamespaceOperation {
    pub fn parse(
        kind: NamespaceOperationKind,
        path: &str,
        destination: Option<&str>,
    ) -> crate::Result<Self> {
        match (kind, destination) {
            (NamespaceOperationKind::Mkdir, None) => Ok(Self::Mkdir {
                path: name_policy::PortablePath::parse(path)?,
            }),
            (NamespaceOperationKind::Delete, None) => Ok(Self::Delete {
                path: name_policy::LookupPath::parse(path)?,
            }),
            (NamespaceOperationKind::DeleteTree, None) => Ok(Self::DeleteTree {
                path: name_policy::LookupPath::parse(path)?,
            }),
            (NamespaceOperationKind::Rename, Some(destination)) => Ok(Self::Rename {
                source: name_policy::LookupPath::parse(path)?,
                destination: name_policy::PortablePath::parse(destination)?,
            }),
            (NamespaceOperationKind::Rename, None) => Err(crate::Error::new(
                crate::ErrorCode::InvalidRequest,
                "rename needs destination",
            )),
            (
                NamespaceOperationKind::Mkdir
                | NamespaceOperationKind::Delete
                | NamespaceOperationKind::DeleteTree,
                Some(_),
            ) => Err(crate::Error::new(
                crate::ErrorCode::InvalidRequest,
                "only rename accepts destination",
            )),
        }
    }
    pub fn kind(&self) -> NamespaceOperationKind {
        match self {
            Self::Mkdir { .. } => NamespaceOperationKind::Mkdir,
            Self::Delete { .. } => NamespaceOperationKind::Delete,
            Self::DeleteTree { .. } => NamespaceOperationKind::DeleteTree,
            Self::Rename { .. } => NamespaceOperationKind::Rename,
        }
    }
    pub fn path(&self) -> &str {
        match self {
            Self::Mkdir { path } => path.as_str(),
            Self::Delete { path }
            | Self::DeleteTree { path }
            | Self::Rename { source: path, .. } => path.as_str(),
        }
    }
    pub fn key(&self) -> &str {
        match self {
            Self::Mkdir { path } => path.key(),
            Self::Delete { path }
            | Self::DeleteTree { path }
            | Self::Rename { source: path, .. } => path.key(),
        }
    }
    pub fn destination(&self) -> Option<&name_policy::PortablePath> {
        match self {
            Self::Mkdir { .. } | Self::Delete { .. } | Self::DeleteTree { .. } => None,
            Self::Rename { destination, .. } => Some(destination),
        }
    }
}
impl Serialize for NamespaceOperation {
    fn serialize<S: data_serialization::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        use data_serialization::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("NamespaceOperation", 3)?;
        state.serialize_field("kind", &self.kind())?;
        state.serialize_field("path", self.path())?;
        state.serialize_field("destination", &self.destination())?;
        state.end()
    }
}
impl<'de> Deserialize<'de> for NamespaceOperation {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serialization(deny_unknown_fields)]
        struct Raw {
            kind: NamespaceOperationKind,
            path: String,
            destination: Option<String>,
        }
        let raw = Raw::deserialize(deserializer)?;
        Self::parse(raw.kind, &raw.path, raw.destination.as_deref())
            .map_err(data_serialization::de::Error::custom)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Workspace {
    pub workspace_id: String,
    pub purpose: WorkspacePurpose,
    pub branch_id: String,
    /// Immutable operation actor, bound to staged data and replay, not an ACL.
    pub principal: String,
    pub base_head: String,
    #[serialization(with = "crate::u64_string")]
    pub fencing_token: u64,
    pub files: BTreeMap<String, StagedFile>,
    /// Freeze the file selections visible when a directory workspace opens.
    /// Two unwritten branches can share a source version but are distinct
    /// destinations, so comparing version IDs alone cannot fence a switch.
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub selected_file_branches: BTreeMap<String, String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub selected_file_contexts: BTreeMap<String, FileContext>,
    /// Selected composition identities at open time; selection is a write even
    /// when two directory snapshots happen to expose identical file bytes.
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub selected_directory_branches: BTreeMap<String, String>,
    #[serialization(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        with = "crate::u64_string_map"
    )]
    pub directory_selection_generations: BTreeMap<String, u64>,
    #[serialization(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        with = "crate::u64_string_map"
    )]
    pub file_selection_generations: BTreeMap<String, u64>,
    pub namespace_operations: Vec<NamespaceOperation>,
    #[serialization(with = "crate::u64_string")]
    pub created_at: u64,
    pub state: WorkspaceState,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct CommitRequest {
    pub workspace_id: String,
    pub operation_id: crate::OperationId,
    pub principal: String,
    pub expected_head: String,
    #[serialization(with = "crate::u64_string")]
    pub fencing_token: u64,
    pub message: String,
    #[serialization(default)]
    pub request_digest: Option<String>,
}
/// Stored logical content disposition, not a live replica-health observation.
/// Exact variant names participate in snapshot and history-package hashes.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub enum ContentState {
    #[serialization(rename = "AVAILABLE")]
    Available,
    #[serialization(rename = "PURGED")]
    Purged,
    #[serialization(rename = "MIGRATED")]
    Migrated,
}
impl<'de> Deserialize<'de> for ContentState {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        // Deriving a unit enum would also accept {"AVAILABLE": null}. Only a
        // string is valid; borrow its bytes instead of allocating per record.
        struct ContentStateVisitor;
        impl data_serialization::de::Visitor<'_> for ContentStateVisitor {
            type Value = ContentState;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("AVAILABLE, PURGED or MIGRATED as a string")
            }

            fn visit_str<E: data_serialization::de::Error>(
                self,
                token: &str,
            ) -> Result<Self::Value, E> {
                match token {
                    "AVAILABLE" => Ok(ContentState::Available),
                    "PURGED" => Ok(ContentState::Purged),
                    "MIGRATED" => Ok(ContentState::Migrated),
                    _ => Err(E::unknown_variant(
                        token,
                        &["AVAILABLE", "PURGED", "MIGRATED"],
                    )),
                }
            }
        }
        deserializer.deserialize_str(ContentStateVisitor)
    }
}
impl ContentState {
    /// The canonical stored token, also used to compare raw proof-page values.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Available => "AVAILABLE",
            Self::Purged => "PURGED",
            Self::Migrated => "MIGRATED",
        }
    }
}

/// Closed publication lifecycle vocabulary shared by persisted and wire results.
/// Serde names are part of the hashed on-disk contract.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serialization(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CommitState {
    Staging,
    Sealed,
    Prepared,
    Deciding,
    Decided,
    Protected,
    Published,
    Acked,
    Recovering,
    RecoveryRequired,
    InDoubt,
    PublishedDegraded,
    Aborted,
}

strict_string_deserialize!(CommitState {
    Staging => "STAGING", Sealed => "SEALED", Prepared => "PREPARED", Deciding => "DECIDING",
    Decided => "DECIDED", Protected => "PROTECTED", Published => "PUBLISHED", Acked => "ACKED",
    Recovering => "RECOVERING", RecoveryRequired => "RECOVERY_REQUIRED", InDoubt => "IN_DOUBT",
    PublishedDegraded => "PUBLISHED_DEGRADED", Aborted => "ABORTED"
});

/// Frozen evidence at publication, not a live assessment of copy health.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serialization(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProtectionAtPublish {
    NotApplicable,
    TargetMet,
    BelowTarget,
    Unconfirmed,
}

strict_string_deserialize!(ProtectionAtPublish {
    NotApplicable => "NOT_APPLICABLE", TargetMet => "TARGET_MET", BelowTarget => "BELOW_TARGET", Unconfirmed => "UNCONFIRMED"
});

/// Current content observation, separate from frozen publication evidence.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serialization(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CurrentHealth {
    Healthy,
    Degraded,
    Unavailable,
    Unknown,
    Recovering,
}

impl<'de> Deserialize<'de> for CurrentHealth {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        struct HealthVisitor;
        impl data_serialization::de::Visitor<'_> for HealthVisitor {
            type Value = CurrentHealth;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a current health string")
            }
            fn visit_str<E: data_serialization::de::Error>(
                self,
                token: &str,
            ) -> Result<Self::Value, E> {
                match token {
                    "HEALTHY" => Ok(CurrentHealth::Healthy),
                    "DEGRADED" => Ok(CurrentHealth::Degraded),
                    "UNAVAILABLE" => Ok(CurrentHealth::Unavailable),
                    "UNKNOWN" => Ok(CurrentHealth::Unknown),
                    "RECOVERING" => Ok(CurrentHealth::Recovering),
                    _ => Err(E::unknown_variant(
                        token,
                        &[
                            "HEALTHY",
                            "DEGRADED",
                            "UNAVAILABLE",
                            "UNKNOWN",
                            "RECOVERING",
                        ],
                    )),
                }
            }
        }
        deserializer.deserialize_str(HealthVisitor)
    }
}

impl CurrentHealth {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "HEALTHY",
            Self::Degraded => "DEGRADED",
            Self::Unavailable => "UNAVAILABLE",
            Self::Unknown => "UNKNOWN",
            Self::Recovering => "RECOVERING",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct OperationResult {
    pub operation_id: crate::OperationId,
    pub kind: String,
    pub commit_id: Option<String>,
    pub history_tree_id: Option<String>,
    pub branch_id: Option<String>,
    pub commit_state: CommitState,
    pub protection_at_publish: ProtectionAtPublish,
    pub current_health: CurrentHealth,
    pub request_digest: String,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    pub result: json_codec::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct Plan {
    pub plan_id: String,
    pub kind: String,
    pub principal: String,
    #[serialization(with = "crate::u64_string")]
    pub expected_sequence: u64,
    pub digest: String,
    pub input: json_codec::Value,
    pub state: PlanState,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct PlacementRule {
    pub rule_id: String,
    pub repository_id: String,
    pub path_prefix: String,
    pub primary_slot_id: String,
    #[serialization(with = "crate::u64_string")]
    pub generation: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct ProtectionPolicy {
    pub policy_id: String,
    pub repository_id: String,
    pub path_prefix: String,
    pub target_copies: u32,
    /// Retained settings apply only to explicit file bindings, not new paths.
    pub inheritable: bool,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    pub scope_directory_id: Option<String>,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    pub scope_file_id: Option<String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub backup_prefixes: BTreeMap<String, String>,
    #[serialization(with = "crate::u64_string")]
    pub generation: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct State {
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    #[serialization(with = "crate::u64_string")]
    pub business_generation: u64,
    pub repositories: crate::TrackedMap<Repository>,
    pub histories: crate::TrackedMap<HistoryTree>,
    pub branches: crate::TrackedMap<Branch>,
    /// Selected independent content branch for each file in its disk namespace.
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_selections: crate::TrackedMap<String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub directory_branches: crate::TrackedMap<DirectoryBranch>,
    #[serialization(default, skip_serializing_if = "SharedRecords::is_empty")]
    pub directory_trees: crate::TrackedRecords<DirectoryTree>,
    /// Current projection intent, separate from each public file branch HEAD.
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_write_origins: crate::TrackedMap<String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_contexts: crate::TrackedMap<FileContext>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_path_nodes: crate::TrackedMap<FilePathNode>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_metadata_nodes: crate::TrackedMap<FileMetadataNode>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_branch_merges: crate::TrackedMap<FileBranchMerge>,
    /// Consumed names remain readable; writes must use the surviving branch.
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_branch_aliases: crate::TrackedMap<String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_node_parents: crate::TrackedMap<Vec<String>>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_path_notes: crate::TrackedMap<String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub directory_tree_selections: crate::TrackedMap<String>,
    /// Physical owner path; selecting an old logical alias never relocates it.
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_locations: crate::TrackedMap<String>,
    /// User-chosen backup owner paths are independent of primary aliases.
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_backup_paths: crate::TrackedMap<BTreeMap<String, String>>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_backup_policies: crate::TrackedMap<String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub directory_selections: crate::TrackedMap<String>,
    #[serialization(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        with = "crate::tracked_u64_string_map"
    )]
    pub directory_selection_generations: crate::TrackedMap<u64>,
    #[serialization(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        with = "crate::tracked_u64_string_map"
    )]
    pub file_selection_generations: crate::TrackedMap<u64>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_notes: crate::TrackedMap<String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub version_notes: crate::TrackedMap<String>,
    #[serialization(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub file_names: crate::TrackedMap<String>,
    pub commits: crate::TrackedRecords<Commit>,
    pub versions: crate::TrackedRecords<FileVersion>,
    pub placements: crate::TrackedMap<Placement>,
    pub merge_links: crate::TrackedMap<MergeLink>,
    pub history_boundaries: crate::TrackedMap<HistoryBoundary>,
    pub content_states: crate::TrackedMap<ContentState>,
    pub operations: crate::TrackedOperations<OperationResult>,
    pub plans: crate::TrackedMap<Plan>,
    pub placement_rules: crate::TrackedMap<PlacementRule>,
    pub protection_policies: crate::TrackedMap<ProtectionPolicy>,
    /// Audit and idempotency owners of published operations, not user accounts.
    pub operation_principals: crate::TrackedOperations<String>,
    /// Private authentication control state; never expose State as a public API DTO.
    #[serialization(default)]
    pub control_auth: Option<json_codec::Value>,
    pub events: crate::TrackedEvents,
    #[serialization(with = "crate::u64_string")]
    pub topology_generation: u64,
}
impl State {
    /// Establish exact per-field provenance for one unpublished transaction.
    pub fn begin_tracking(&mut self) {
        let Self {
            sequence: _,
            business_generation: _,
            topology_generation: _,
            control_auth: _,
            events,
            repositories,
            histories,
            branches,
            file_selections,
            directory_branches,
            directory_trees,
            file_write_origins,
            file_contexts,
            file_path_nodes,
            file_metadata_nodes,
            file_branch_merges,
            file_branch_aliases,
            file_node_parents,
            file_path_notes,
            directory_tree_selections,
            file_locations,
            file_backup_paths,
            file_backup_policies,
            directory_selections,
            directory_selection_generations,
            file_selection_generations,
            file_notes,
            version_notes,
            file_names,
            commits,
            versions,
            placements,
            merge_links,
            history_boundaries,
            content_states,
            operations,
            plans,
            placement_rules,
            protection_policies,
            operation_principals,
        } = self;
        events.begin_tracking();
        repositories.begin_tracking();
        histories.begin_tracking();
        branches.begin_tracking();
        file_selections.begin_tracking();
        directory_branches.begin_tracking();
        directory_trees.begin_tracking();
        file_write_origins.begin_tracking();
        file_contexts.begin_tracking();
        file_path_nodes.begin_tracking();
        file_metadata_nodes.begin_tracking();
        file_branch_merges.begin_tracking();
        file_branch_aliases.begin_tracking();
        file_node_parents.begin_tracking();
        file_path_notes.begin_tracking();
        directory_tree_selections.begin_tracking();
        file_locations.begin_tracking();
        file_backup_paths.begin_tracking();
        file_backup_policies.begin_tracking();
        directory_selections.begin_tracking();
        directory_selection_generations.begin_tracking();
        file_selection_generations.begin_tracking();
        file_notes.begin_tracking();
        version_notes.begin_tracking();
        file_names.begin_tracking();
        commits.begin_tracking();
        versions.begin_tracking();
        placements.begin_tracking();
        merge_links.begin_tracking();
        history_boundaries.begin_tracking();
        content_states.begin_tracking();
        operations.begin_tracking();
        plans.begin_tracking();
        placement_rules.begin_tracking();
        protection_policies.begin_tracking();
        operation_principals.begin_tracking();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub struct Event {
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    pub operation_id: crate::OperationId,
    pub kind: String,
    pub object_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryGraph {
    pub history: HistoryTree,
    pub commits: Vec<Commit>,
    pub branches: Vec<Branch>,
    pub merge_links: Vec<MergeLink>,
    #[serialization(default)]
    pub boundary_marks: Vec<HistoryBoundary>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiskStatus {
    pub slot_id: String,
    pub disk_id: String,
    pub level: u32,
    pub upstream_slot_id: Option<String>,
    pub online: bool,
    pub metadata_member: bool,
    /// Optional diagnostic association, never proof of failure-domain independence.
    pub physical_device: Option<String>,
    #[serialization(with = "crate::u64_string")]
    pub available_bytes: u64,
    #[serialization(with = "crate::u64_string")]
    pub used_bytes: u64,
    pub simulation: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct CheckReport {
    pub checked_versions: usize,
    pub healthy_replicas: usize,
    pub tombstoned_versions: usize,
    pub issues: Vec<String>,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/model/repository_tests.rs"]
mod repository_tests;

#[cfg(test)]
#[path = "../../../test/contract/unit/model/operation_status_tests.rs"]
mod operation_status_tests;

#[cfg(test)]
#[path = "../../../test/contract/unit/model/config_tests.rs"]
mod config_tests;

#[cfg(test)]
#[path = "../../../test/contract/unit/model/domain_state_tests.rs"]
mod domain_state_tests;
