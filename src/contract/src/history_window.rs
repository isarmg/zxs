//! Current-root, request-local history windows. These DTOs confer no authority,
//! disk health, retained-root lease or permission to submit an old write draft.
use crate::{ContentHash, ContentState, Error, ErrorCode, Result};
use data_serialization::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

pub const MAX_HISTORY_RESPONSE_BYTES: usize = crate::client_protocol::MAX_RESPONSE_BYTES;
pub const MAX_HISTORY_CURSOR_BYTES: usize = 4096;
pub const MAX_HISTORY_PREVIEW_EDGES: usize = 256;
pub const MAX_HISTORY_FIELD_BYTES: usize = 65_536;
/// Escaped scalar fields plus one bounded parent cursor; edges have a separate budget.
pub const MAX_HISTORY_NODE_ENCODED_BYTES: usize = 65_536;
pub const MAX_HISTORY_ENVELOPE_BYTES: usize = 65_536;

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $token:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
        #[serialization(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                match String::deserialize(d)?.as_str() {
                    $($token => Ok(Self::$variant),)+
                    value => Err(data_serialization::de::Error::unknown_variant(value, &[$($token),+])),
                }
            }
        }
    };
}

macro_rules! identifier {
    ($name:ident, $($prefix:literal),+) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serialization(transparent)]
        pub struct $name(String);
        impl $name {
            pub fn parse(value: &str) -> Result<Self> {
                if ![$($prefix),+].iter().any(|prefix| crate::validate_id(value, prefix).is_ok()) {
                    return Err(invalid(concat!("invalid ", stringify!($name))));
                }
                Ok(Self(value.into()))
            }
            pub fn as_str(&self) -> &str { &self.0 }
        }
        impl FromStr for $name {
            type Err = Error;
            fn from_str(value: &str) -> Result<Self> { Self::parse(value) }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                Self::parse(&String::deserialize(d)?).map_err(data_serialization::de::Error::custom)
            }
        }
    };
}
identifier!(HistoryFileId, "file");
identifier!(HistoryRepositoryId, "repo");
identifier!(HistoryTreeId, "history");
identifier!(HistoryVersionId, "ver");
identifier!(HistoryNodeId, "ver", "fnode", "fmeta");
identifier!(HistoryBranchId, "br");
identifier!(HistoryMergeId, "fmerge");
identifier!(HistoryCommitId, "commit");

macro_rules! digest {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serialization(transparent)]
        pub struct $name(ContentHash);
        impl $name {
            pub fn parse(value: &str) -> Result<Self> {
                value
                    .parse()
                    .map(Self)
                    .map_err(|_| invalid(concat!("invalid ", stringify!($name))))
            }
            pub fn from_digest(value: blake3::Hash) -> Self {
                Self(ContentHash::from_digest(value))
            }
        }
        impl FromStr for $name {
            type Err = Error;
            fn from_str(value: &str) -> Result<Self> {
                Self::parse(value)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }
    };
}
digest!(HistoryRoot);
digest!(HistoryQueryHash);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryLocation {
    pub repository_id: HistoryRepositoryId,
    pub path: name_policy::LookupPath,
}
impl HistoryLocation {
    pub fn new(repository_id: HistoryRepositoryId, path: name_policy::LookupPath) -> Self {
        Self {
            repository_id,
            path,
        }
    }
}

/// Unbounded persisted scalar fields are read separately without new write limits.
string_enum!(HistoryField { Message => "message", Author => "author", CreatedAt => "created_at" });

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryCollection {
    Nodes,
    Versions,
    Branches,
    Merges,
    NodeParents {
        node_id: HistoryNodeId,
    },
    VersionSources {
        version_id: HistoryVersionId,
    },
    MergeInputs {
        merge_id: HistoryMergeId,
    },
    MergeParents {
        merge_id: HistoryMergeId,
    },
    NodeField {
        node_id: HistoryNodeId,
        field: HistoryField,
    },
}
impl HistoryCollection {
    pub fn maximum_limit(&self) -> usize {
        match self {
            Self::Nodes => 64,
            Self::Versions | Self::Branches | Self::Merges => 128,
            Self::NodeParents { .. }
            | Self::VersionSources { .. }
            | Self::MergeInputs { .. }
            | Self::MergeParents { .. } => 512,
            Self::NodeField { .. } => MAX_HISTORY_FIELD_BYTES,
        }
    }
    pub fn requires_root(&self) -> bool {
        !matches!(self, Self::Nodes | Self::Versions)
    }
}

/// Records per page, or raw UTF-8 bytes for a field fragment. It is not a total-history limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serialization(transparent)]
pub struct HistoryPageLimit(u32);
impl HistoryPageLimit {
    pub fn new(value: usize) -> Result<Self> {
        if value == 0 || value > MAX_HISTORY_FIELD_BYTES {
            return Err(invalid("invalid history page limit"));
        }
        Ok(Self(value as u32))
    }
    pub fn for_collection(value: usize, collection: &HistoryCollection) -> Result<Self> {
        if value > collection.maximum_limit() {
            return Err(invalid("history page limit exceeds collection budget"));
        }
        Self::new(value)
    }
    pub fn default_for(collection: &HistoryCollection) -> Self {
        Self((collection.maximum_limit() / 2) as u32)
    }
    pub fn get(self) -> usize {
        self.0 as usize
    }
}
impl<'de> Deserialize<'de> for HistoryPageLimit {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::new(u32::deserialize(d)? as usize).map_err(data_serialization::de::Error::custom)
    }
}

string_enum!(HistoryCursorFormat { HistoryWindowV1 => "history_window_v1" });
string_enum!(HistoryDirection { Forward => "forward", Backward => "backward" });
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryPosition {
    /// Inclusive beginning for a parent's first page when preview allocated no edges.
    Start,
    Node {
        #[serialization(with = "crate::u64_string")]
        sequence: u64,
        node_id: HistoryNodeId,
    },
    Version {
        #[serialization(with = "crate::u64_string")]
        sequence: u64,
        version_id: HistoryVersionId,
    },
    Branch {
        branch_id: HistoryBranchId,
    },
    Merge {
        #[serialization(with = "crate::u64_string")]
        sequence: u64,
        merge_id: HistoryMergeId,
    },
    Parent {
        parent_node_id: HistoryNodeId,
    },
    Member {
        #[serialization(with = "crate::u64_string")]
        ordinal: u64,
    },
    Field {
        #[serialization(with = "crate::u64_string")]
        byte_offset: u64,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryCursorPayload {
    pub format: HistoryCursorFormat,
    pub history_root: HistoryRoot,
    pub history_tree_id: HistoryTreeId,
    pub file_id: HistoryFileId,
    pub collection: HistoryCollection,
    pub query_hash: HistoryQueryHash,
    pub limit: HistoryPageLimit,
    pub direction: HistoryDirection,
    pub position: HistoryPosition,
    pub anchor_node_id: Option<HistoryNodeId>,
}
impl HistoryCursorPayload {
    pub fn validate(&self) -> Result<()> {
        HistoryPageLimit::for_collection(self.limit.get(), &self.collection)?;
        let compatible = (matches!(
            (&self.collection, &self.position),
            (
                HistoryCollection::NodeParents { .. },
                HistoryPosition::Start
            )
        ) && self.direction == HistoryDirection::Forward)
            || matches!(
                (&self.collection, &self.position),
                (HistoryCollection::Nodes, HistoryPosition::Node { .. })
                    | (HistoryCollection::Versions, HistoryPosition::Version { .. })
                    | (HistoryCollection::Branches, HistoryPosition::Branch { .. })
                    | (HistoryCollection::Merges, HistoryPosition::Merge { .. })
                    | (
                        HistoryCollection::NodeParents { .. },
                        HistoryPosition::Parent { .. }
                    )
                    | (
                        HistoryCollection::VersionSources { .. }
                            | HistoryCollection::MergeInputs { .. }
                            | HistoryCollection::MergeParents { .. },
                        HistoryPosition::Member { .. }
                    )
                    | (
                        HistoryCollection::NodeField { .. },
                        HistoryPosition::Field { .. }
                    )
            );
        if !compatible
            || (self.anchor_node_id.is_some() && self.collection != HistoryCollection::Nodes)
        {
            return Err(invalid(
                "history cursor position does not match its collection",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryCursor {
    encoded: String,
    payload: Box<HistoryCursorPayload>,
}
impl HistoryCursor {
    pub fn parse(encoded: &str) -> Result<Self> {
        if encoded.is_empty()
            || encoded.len() > MAX_HISTORY_CURSOR_BYTES
            || !encoded.len().is_multiple_of(2)
        {
            return Err(invalid("invalid history cursor length"));
        }
        let bytes = encoded
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                let digit = |byte| match byte {
                    b'0'..=b'9' => Ok(byte - b'0'),
                    b'a'..=b'f' => Ok(byte - b'a' + 10),
                    _ => Err(invalid("invalid history cursor encoding")),
                };
                Ok(digit(pair[0])? * 16 + digit(pair[1])?)
            })
            .collect::<Result<Vec<u8>>>()?;
        let payload: HistoryCursorPayload = crate::decode_strict(&bytes)?;
        payload.validate()?;
        Ok(Self {
            encoded: encoded.into(),
            payload: Box::new(payload),
        })
    }
    pub fn from_payload(payload: HistoryCursorPayload) -> Result<Self> {
        payload.validate()?;
        let bytes = json_codec::to_vec(&payload)?;
        if bytes.len() > MAX_HISTORY_CURSOR_BYTES / 2 {
            return Err(invalid("history cursor exceeds budget"));
        }
        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write;
            write!(&mut encoded, "{byte:02x}").expect("invariant: writing to String succeeds");
        }
        Ok(Self {
            encoded,
            payload: Box::new(payload),
        })
    }
    pub fn as_str(&self) -> &str {
        &self.encoded
    }
    pub fn payload(&self) -> &HistoryCursorPayload {
        &self.payload
    }
}
impl Serialize for HistoryCursor {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.encoded)
    }
}
impl<'de> Deserialize<'de> for HistoryCursor {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::parse(&String::deserialize(d)?).map_err(data_serialization::de::Error::custom)
    }
}

#[derive(Clone, Debug)]
pub struct HistoryWindowRequest {
    pub file_id: HistoryFileId,
    pub collection: HistoryCollection,
    pub location: Option<HistoryLocation>,
    pub history_root: Option<HistoryRoot>,
    pub cursor: Option<HistoryCursor>,
    pub anchor_node_id: Option<HistoryNodeId>,
    pub limit: HistoryPageLimit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryResolvedContext {
    pub repository_id: HistoryRepositoryId,
    pub history_tree_id: HistoryTreeId,
    pub path: Option<name_policy::PortablePath>,
    pub current_node_id: Option<HistoryNodeId>,
    pub selected_branch_id: Option<HistoryBranchId>,
}
impl HistoryWindowRequest {
    /// Call only after current ownership/context resolution. The digest binds
    /// normalized requested and actual context without putting paths in cursors.
    pub fn query_hash(
        &self,
        resolved: &HistoryResolvedContext,
        anchor: Option<&HistoryNodeId>,
    ) -> HistoryQueryHash {
        let requested = self.location.as_ref().map(|location| {
            json_codec::json!({
                "repository_id":location.repository_id, "path_key":location.path.key()
            })
        });
        let binding = json_codec::json!({"policy":"history_window_v1", "file_id":self.file_id,
            "collection":self.collection,"limit":self.limit,"anchor":anchor,"requested":requested,
            "resolved":{"repository_id":resolved.repository_id,"history_tree_id":resolved.history_tree_id,
                "path_key":resolved.path.as_ref().map(name_policy::PortablePath::key),
                "current_node_id":resolved.current_node_id,"selected_branch_id":resolved.selected_branch_id}});
        HistoryQueryHash::from_digest(blake3::hash(
            &json_codec::to_vec(&binding)
                .expect("invariant: internal history query binding serializes"),
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryPageIdentity {
    pub history_root: HistoryRoot,
    pub file_id: HistoryFileId,
    pub collection: HistoryCollection,
    pub context: HistoryResolvedContext,
    pub query_hash: HistoryQueryHash,
    pub limit: HistoryPageLimit,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryPageCursors {
    pub previous_cursor: Option<HistoryCursor>,
    pub next_cursor: Option<HistoryCursor>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryContentSource {
    Owned,
    Inherited {
        source_version_id: Option<HistoryVersionId>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryContentDisposition {
    pub version_id: Option<HistoryVersionId>,
    pub source: HistoryContentSource,
    pub stored_state: Option<ContentState>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryContextSummary {
    pub note: String,
    pub content: Option<HistoryContentDisposition>,
}
string_enum!(HistoryNodeKind { Content => "content", Rename => "rename", Tags => "tags", Merge => "merge" });
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryParentSet {
    Complete,
    More { cursor: HistoryCursor },
}
string_enum!(HistoryEndpoint { InWindow => "in_window", OutsideWindow => "outside_window" });
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryGraphEdge {
    pub node_id: HistoryNodeId,
    pub parent_node_id: HistoryNodeId,
    pub endpoint: HistoryEndpoint,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryFieldInfo {
    #[serialization(with = "crate::u64_string")]
    pub byte_length: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryNodeFields {
    pub message: HistoryFieldInfo,
    pub author: HistoryFieldInfo,
    pub created_at: HistoryFieldInfo,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryNodeSummary {
    pub node_id: HistoryNodeId,
    pub version_id: HistoryVersionId,
    pub kind: HistoryNodeKind,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    pub current: bool,
    pub branch_id: Option<HistoryBranchId>,
    pub path: String,
    pub previous_path: Option<String>,
    pub note: String,
    pub tags: Vec<String>,
    #[serialization(with = "crate::u64_string")]
    pub size: u64,
    pub content_hash: ContentHash,
    pub content_state: Option<ContentState>,
    pub download_name: String,
    pub storage_name: String,
    pub parents: HistoryParentSet,
    pub fields: HistoryNodeFields,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryVersionSummary {
    pub version_id: HistoryVersionId,
    #[serialization(with = "crate::u64_string")]
    pub version_sequence: u64,
    #[serialization(with = "crate::u64_string")]
    pub size: u64,
    pub content_hash: ContentHash,
    pub content_state: Option<ContentState>,
    pub note: String,
    pub download_name: String,
    pub storage_name: String,
    #[serialization(with = "crate::u64_string")]
    pub source_count: u64,
    pub fields: HistoryNodeFields,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryBranchSummary {
    pub branch_id: HistoryBranchId,
    pub name: String,
    pub head_commit_id: HistoryCommitId,
    pub fork_commit_id: HistoryCommitId,
    pub node_id: HistoryNodeId,
    pub state: crate::BranchState,
    pub branch_kind: crate::BranchKind,
    #[serialization(with = "crate::u64_string")]
    pub generation: u64,
    pub current: bool,
    pub read_only: bool,
    pub alias_of: Option<HistoryBranchId>,
    pub content: HistoryContentDisposition,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryMergeSummary {
    pub merge_id: HistoryMergeId,
    #[serialization(with = "crate::u64_string")]
    pub sequence: u64,
    pub chosen_node_id: HistoryNodeId,
    pub result_node_id: HistoryNodeId,
    pub surviving_branch_id: HistoryBranchId,
    pub reused_node: bool,
    pub operation_id: crate::OperationId,
    #[serialization(with = "crate::u64_string")]
    pub input_count: u64,
    #[serialization(with = "crate::u64_string")]
    pub parent_count: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryParentMember {
    pub parent_node_id: HistoryNodeId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistorySourceMember {
    #[serialization(with = "crate::u64_string")]
    pub ordinal: u64,
    pub version_id: HistoryVersionId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryMergeInputMember {
    #[serialization(with = "crate::u64_string")]
    pub ordinal: u64,
    pub branch_id: HistoryBranchId,
    pub node_id: HistoryNodeId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct HistoryMergeParentMember {
    #[serialization(with = "crate::u64_string")]
    pub ordinal: u64,
    pub parent_node_id: HistoryNodeId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryWindowPage {
    Nodes {
        identity: HistoryPageIdentity,
        context: HistoryContextSummary,
        nodes: Vec<HistoryNodeSummary>,
        edges: Vec<HistoryGraphEdge>,
        page: HistoryPageCursors,
    },
    Versions {
        identity: HistoryPageIdentity,
        versions: Vec<HistoryVersionSummary>,
        page: HistoryPageCursors,
    },
    Branches {
        identity: HistoryPageIdentity,
        branches: Vec<HistoryBranchSummary>,
        page: HistoryPageCursors,
    },
    Merges {
        identity: HistoryPageIdentity,
        merges: Vec<HistoryMergeSummary>,
        page: HistoryPageCursors,
    },
    NodeParents {
        identity: HistoryPageIdentity,
        parents: Vec<HistoryParentMember>,
        page: HistoryPageCursors,
    },
    VersionSources {
        identity: HistoryPageIdentity,
        sources: Vec<HistorySourceMember>,
        page: HistoryPageCursors,
    },
    MergeInputs {
        identity: HistoryPageIdentity,
        inputs: Vec<HistoryMergeInputMember>,
        page: HistoryPageCursors,
    },
    MergeParents {
        identity: HistoryPageIdentity,
        parents: Vec<HistoryMergeParentMember>,
        page: HistoryPageCursors,
    },
    /// end_of_field describes this fragment's right boundary, never ownership of
    /// all previous fragments. Empty fields return offset=total=0 and text="".
    NodeField {
        identity: HistoryPageIdentity,
        field: HistoryField,
        #[serialization(with = "crate::u64_string")]
        total_bytes: u64,
        #[serialization(with = "crate::u64_string")]
        byte_offset: u64,
        text: String,
        end_of_field: bool,
        next_cursor: Option<HistoryCursor>,
    },
}
