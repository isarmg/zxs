//! Bounded file-history reads: typed requests, cursors, page models and the
//! byte-budgeted wire encoder. These observations grant no later authorization.
use crate::{ApiError, ContentDispositionSnapshot};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{BranchKind, BranchState, ContentHash, ContentState, OperationId};

pub(crate) mod cursors;
mod wire;
pub use cursors::{
    BranchCursor, BranchCursorKey, CursorKind, HistoryCursorSeed, MergeCursor, MergeCursorKey,
    NodeCursor, NodeCursorKey, ParentCursor, ParentCursorKey, VersionCursor, VersionCursorKey,
};
pub(crate) use wire::{EncodedPage, PageBudget, encode_bounded};

/// Page record limits per collection. These bound one response, not the history.
pub mod limits {
    pub const NODES_DEFAULT: usize = 50;
    pub const NODES_MAX: usize = 200;
    pub const PARENTS_DEFAULT: usize = 100;
    pub const PARENTS_MAX: usize = 500;
    pub const BRANCHES_DEFAULT: usize = 100;
    pub const BRANCHES_MAX: usize = 500;
    pub const MERGES_DEFAULT: usize = 50;
    pub const MERGES_MAX: usize = 200;
    pub const SLICE_DEFAULT: usize = 200;
    pub const SLICE_MAX: usize = 1000;
    pub const VERSIONS_DEFAULT: usize = 100;
    pub const VERSIONS_MAX: usize = 500;
    pub const FIELD_DEFAULT_BYTES: usize = 64 * 1024;
    pub const FIELD_MAX_BYTES: usize = 1024 * 1024;
    /// Parent edges enumerated inline per node of a node page.
    pub const MAX_PARENT_PREVIEW_PER_NODE: usize = 32;
    /// Parent edges enumerated inline across one node page.
    pub const MAX_PAGE_PARENT_EDGES: usize = 2048;
    /// Longest cursor accepted or produced, in hexadecimal characters.
    pub const MAX_CURSOR_CHARS: usize = 32_768;
    /// Longest identifier accepted on a path or in a cursor, in UTF-8 bytes.
    pub const MAX_ID_BYTES: usize = 256;
}

/// A non-empty, bounded identifier from a request path or cursor. Format
/// validity against State is decided by the backend, never by this parser.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestId(String);
impl RequestId {
    pub fn parse(field: &'static str, value: String) -> Result<Self, ApiError> {
        if value.is_empty() {
            return Err(ApiError::invalid(format!("missing string field {field}")));
        }
        if value.len() > limits::MAX_ID_BYTES {
            return Err(ApiError::invalid(format!(
                "{field} exceeds {} UTF-8 bytes",
                limits::MAX_ID_BYTES
            )));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

/// Exactly 64 lowercase hex digits naming a published history manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRootText(String);
impl HistoryRootText {
    pub fn parse(value: &str) -> Result<Self, ApiError> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ApiError::invalid(
                "history_root must be 64 lowercase hexadecimal digits",
            ));
        }
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Query fields with strict membership: unknown keys are rejected before any
/// value is interpreted, matching the other typed read routes.
pub struct QueryFields(BTreeMap<String, String>);
impl QueryFields {
    pub fn strict(values: BTreeMap<String, String>, allowed: &[&str]) -> Result<Self, ApiError> {
        if let Some(key) = values.keys().find(|key| !allowed.contains(&key.as_str())) {
            return Err(ApiError::invalid(format!("unknown field {key}")));
        }
        Ok(Self(values))
    }
    pub fn take(&mut self, key: &str) -> Option<String> {
        self.0.remove(key)
    }
    pub fn limit(&mut self, default: usize, maximum: usize) -> Result<usize, ApiError> {
        let Some(value) = self.0.remove("limit") else {
            return Ok(default);
        };
        let limit = storage_contract::parse_u64_decimal(&value)
            .map_err(ApiError::contract_input)
            .and_then(|limit| usize::try_from(limit).map_err(ApiError::invalid_source))?;
        if !(1..=maximum).contains(&limit) {
            return Err(ApiError::invalid(format!(
                "limit must be between 1 and {maximum}"
            )));
        }
        Ok(limit)
    }
    pub fn offset(&mut self) -> Result<usize, ApiError> {
        let Some(value) = self.0.remove("offset") else {
            return Ok(0);
        };
        storage_contract::parse_u64_decimal(&value)
            .map_err(ApiError::contract_input)
            .and_then(|offset| usize::try_from(offset).map_err(ApiError::invalid_source))
    }
    pub fn root(&mut self) -> Result<Option<HistoryRootText>, ApiError> {
        self.0
            .remove("history_root")
            .map(|value| HistoryRootText::parse(&value))
            .transpose()
    }
    pub fn cursor<C: cursors::HistoryCursor>(
        &mut self,
        file_id: &RequestId,
    ) -> Result<Option<C>, ApiError> {
        self.0
            .remove("cursor")
            .map(|value| cursors::decode_bound(&value, file_id.as_str()))
            .transpose()
    }
}

/// The root a request observes: an explicit query value, a cursor's, or both
/// when they agree. A disagreement is a malformed request, not a stale page.
pub fn observed_root(
    explicit: Option<HistoryRootText>,
    cursor: Option<&str>,
) -> Result<Option<HistoryRootText>, ApiError> {
    match (explicit, cursor) {
        (Some(explicit), Some(cursor)) if explicit.as_str() != cursor => Err(ApiError::invalid(
            "history_root differs from the cursor's history root",
        )),
        (Some(explicit), _) => Ok(Some(explicit)),
        (None, Some(cursor)) => Ok(Some(HistoryRootText::parse(cursor)?)),
        (None, None) => Ok(None),
    }
}

pub fn required_root(root: Option<HistoryRootText>) -> Result<HistoryRootText, ApiError> {
    root.ok_or_else(|| ApiError::invalid("history_root is required (query or cursor)"))
}

/// Explicit lookup context for the main history page.
#[derive(Debug)]
pub struct FileHistoryLocation {
    pub(crate) repository_id: String,
    pub(crate) path: name_policy::LookupPath,
}
impl FileHistoryLocation {
    pub fn parse(
        repository_id: Option<String>,
        path: Option<String>,
    ) -> Result<Option<Self>, ApiError> {
        match (repository_id, path) {
            (None, None) => Ok(None),
            (Some(repository_id), Some(path)) => Ok(Some(Self {
                repository_id,
                path: name_policy::LookupPath::parse(&path).map_err(ApiError::invalid_source)?,
            })),
            (Some(_), None) | (None, Some(_)) => Err(ApiError::invalid(
                "repository_id and path must be supplied together",
            )),
        }
    }
}

/// Where a node page starts.
#[derive(Debug)]
pub enum NodePosition {
    First,
    Anchor(RequestId),
    Cursor(NodeCursor),
}

/// `GET /api/file-manager/files/{file}/history`.
#[derive(Debug)]
pub struct ReadFileHistory {
    pub(crate) file_id: RequestId,
    pub(crate) location: Option<FileHistoryLocation>,
    pub(crate) limit: usize,
    pub(crate) position: NodePosition,
    pub(crate) root: Option<HistoryRootText>,
}
impl ReadFileHistory {
    pub fn parse(file_id: String, query: BTreeMap<String, String>) -> Result<Self, ApiError> {
        let file_id = RequestId::parse("file_id", file_id)?;
        let mut query = QueryFields::strict(
            query,
            &[
                "repository_id",
                "path",
                "limit",
                "cursor",
                "anchor_node_id",
                "history_root",
            ],
        )?;
        let location = FileHistoryLocation::parse(query.take("repository_id"), query.take("path"))?;
        let limit = query.limit(limits::NODES_DEFAULT, limits::NODES_MAX)?;
        let cursor: Option<NodeCursor> = query.cursor(&file_id)?;
        let anchor = query.take("anchor_node_id");
        let root = observed_root(query.root()?, cursor.as_ref().map(NodeCursor::root))?;
        let position = match (cursor, anchor) {
            (Some(_), Some(_)) => {
                return Err(ApiError::invalid(
                    "cursor and anchor_node_id are mutually exclusive",
                ));
            }
            (Some(cursor), None) => NodePosition::Cursor(cursor),
            (None, Some(anchor)) => {
                if root.is_none() {
                    return Err(ApiError::invalid("anchor_node_id requires history_root"));
                }
                NodePosition::Anchor(RequestId::parse("anchor_node_id", anchor)?)
            }
            (None, None) => NodePosition::First,
        };
        Ok(Self {
            file_id,
            location,
            limit,
            position,
            root,
        })
    }
    pub fn file_id(&self) -> &str {
        self.file_id.as_str()
    }
    pub fn location(&self) -> Option<(&str, &name_policy::LookupPath)> {
        self.location
            .as_ref()
            .map(|location| (location.repository_id.as_str(), &location.path))
    }
}

/// A cursor-paged sub-collection request: `{file}` plus optional subject id.
#[derive(Debug)]
pub struct ReadHistoryCollection<C> {
    pub(crate) file_id: RequestId,
    pub(crate) limit: usize,
    pub(crate) cursor: Option<C>,
    pub(crate) root: HistoryRootText,
}

fn parse_collection<C: cursors::HistoryCursor>(
    file_id: String,
    query: BTreeMap<String, String>,
    default: usize,
    maximum: usize,
    cursor_root: impl Fn(&C) -> &str,
) -> Result<ReadHistoryCollection<C>, ApiError> {
    let file_id = RequestId::parse("file_id", file_id)?;
    let mut query = QueryFields::strict(query, &["limit", "cursor", "history_root"])?;
    let limit = query.limit(default, maximum)?;
    let cursor: Option<C> = query.cursor(&file_id)?;
    let root = required_root(observed_root(
        query.root()?,
        cursor.as_ref().map(&cursor_root),
    )?)?;
    Ok(ReadHistoryCollection {
        file_id,
        limit,
        cursor,
        root,
    })
}

/// `GET /api/file-manager/files/{file}/history/nodes/{node}/parents`.
#[derive(Debug)]
pub struct ReadNodeParents {
    pub(crate) node_id: RequestId,
    pub(crate) collection: ReadHistoryCollection<ParentCursor>,
}
impl ReadNodeParents {
    pub fn parse(
        file_id: String,
        node_id: String,
        query: BTreeMap<String, String>,
    ) -> Result<Self, ApiError> {
        Ok(Self {
            node_id: RequestId::parse("node_id", node_id)?,
            collection: parse_collection(
                file_id,
                query,
                limits::PARENTS_DEFAULT,
                limits::PARENTS_MAX,
                ParentCursor::root,
            )?,
        })
    }
}

/// `GET /api/file-manager/files/{file}/history/branches`.
#[derive(Debug)]
pub struct ReadHistoryBranches(pub ReadHistoryCollection<BranchCursor>);
impl ReadHistoryBranches {
    pub fn parse(file_id: String, query: BTreeMap<String, String>) -> Result<Self, ApiError> {
        parse_collection(
            file_id,
            query,
            limits::BRANCHES_DEFAULT,
            limits::BRANCHES_MAX,
            BranchCursor::root,
        )
        .map(Self)
    }
}

/// `GET /api/file-manager/files/{file}/history/merges`.
#[derive(Debug)]
pub struct ReadHistoryMerges(pub ReadHistoryCollection<MergeCursor>);
impl ReadHistoryMerges {
    pub fn parse(file_id: String, query: BTreeMap<String, String>) -> Result<Self, ApiError> {
        parse_collection(
            file_id,
            query,
            limits::MERGES_DEFAULT,
            limits::MERGES_MAX,
            MergeCursor::root,
        )
        .map(Self)
    }
}

/// `GET /api/files/{file}/history`.
#[derive(Debug)]
pub struct ReadVersionPage {
    pub(crate) file_id: RequestId,
    pub(crate) limit: usize,
    pub(crate) cursor: Option<VersionCursor>,
    pub(crate) root: Option<HistoryRootText>,
}
impl ReadVersionPage {
    pub fn parse(file_id: String, query: BTreeMap<String, String>) -> Result<Self, ApiError> {
        let file_id = RequestId::parse("file_id", file_id)?;
        let mut query = QueryFields::strict(query, &["limit", "cursor", "history_root"])?;
        let limit = query.limit(limits::VERSIONS_DEFAULT, limits::VERSIONS_MAX)?;
        let cursor: Option<VersionCursor> = query.cursor(&file_id)?;
        // The first body page may capture the root; continuations carry it.
        let root = observed_root(query.root()?, cursor.as_ref().map(VersionCursor::root))?;
        Ok(Self {
            file_id,
            limit,
            cursor,
            root,
        })
    }
    /// None on an unrooted first page.
    pub fn root(&self) -> Option<&HistoryRootText> {
        self.root.as_ref()
    }
}

/// An offset slice request over an index-addressed relation of one subject.
#[derive(Debug)]
pub struct ReadIndexedSlice {
    pub(crate) file_id: RequestId,
    pub(crate) subject_id: RequestId,
    pub(crate) offset: usize,
    pub(crate) limit: usize,
    pub(crate) root: HistoryRootText,
}
impl ReadIndexedSlice {
    pub fn parse(
        file_id: String,
        subject: &'static str,
        subject_id: String,
        query: BTreeMap<String, String>,
    ) -> Result<Self, ApiError> {
        let file_id = RequestId::parse("file_id", file_id)?;
        let subject_id = RequestId::parse(subject, subject_id)?;
        let mut query = QueryFields::strict(query, &["offset", "limit", "history_root"])?;
        let offset = query.offset()?;
        let limit = query.limit(limits::SLICE_DEFAULT, limits::SLICE_MAX)?;
        let root = required_root(query.root()?)?;
        Ok(Self {
            file_id,
            subject_id,
            offset,
            limit,
            root,
        })
    }
}

/// `GET .../merges/{merge}/inputs`.
#[derive(Debug)]
pub struct ReadMergeInputs(pub ReadIndexedSlice);
/// `GET .../merges/{merge}/parents`.
#[derive(Debug)]
pub struct ReadMergeParents(pub ReadIndexedSlice);
/// `GET /api/files/{file}/history/versions/{version}/sources`.
#[derive(Debug)]
pub struct ReadVersionSources(pub ReadIndexedSlice);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeFieldName {
    Message,
    Author,
}
impl NodeFieldName {
    pub fn parse(value: &str) -> Result<Self, ApiError> {
        match value {
            "message" => Ok(Self::Message),
            "author" => Ok(Self::Author),
            _ => Err(ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "unknown field",
            )),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Author => "author",
        }
    }
}

/// `GET .../nodes/{node}/fields/{field}`.
#[derive(Debug)]
pub struct ReadNodeField {
    pub(crate) file_id: RequestId,
    pub(crate) node_id: RequestId,
    pub(crate) field: NodeFieldName,
    pub(crate) offset: usize,
    pub(crate) max_bytes: usize,
    pub(crate) root: HistoryRootText,
}
impl ReadNodeField {
    pub fn parse(
        file_id: String,
        node_id: String,
        field: String,
        query: BTreeMap<String, String>,
    ) -> Result<Self, ApiError> {
        let file_id = RequestId::parse("file_id", file_id)?;
        let node_id = RequestId::parse("node_id", node_id)?;
        let field = NodeFieldName::parse(&field)?;
        let mut query = QueryFields::strict(query, &["offset", "max_bytes", "history_root"])?;
        let offset = query.offset()?;
        let max_bytes = match query.take("max_bytes") {
            None => limits::FIELD_DEFAULT_BYTES,
            Some(value) => {
                let bytes = storage_contract::parse_u64_decimal(&value)
                    .map_err(ApiError::contract_input)
                    .and_then(|bytes| usize::try_from(bytes).map_err(ApiError::invalid_source))?;
                if !(1..=limits::FIELD_MAX_BYTES).contains(&bytes) {
                    return Err(ApiError::invalid(format!(
                        "max_bytes must be between 1 and {}",
                        limits::FIELD_MAX_BYTES
                    )));
                }
                bytes
            }
        };
        let root = required_root(query.root()?)?;
        Ok(Self {
            file_id,
            node_id,
            field,
            offset,
            max_bytes,
            root,
        })
    }
}

// Page models captured under admission; wire conversion happens afterwards.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedText {
    pub bytes: usize,
    pub inline: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryNodeKind {
    Content,
    Rename,
    Tags,
    Merge,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParentEdge {
    pub node_id: String,
    pub in_window: bool,
}

/// Explicit parent-set state: `More` always carries a continuation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParentPreview {
    Complete(Vec<ParentEdge>),
    More {
        edges: Vec<ParentEdge>,
        cursor: String,
    },
}
impl ParentPreview {
    pub fn edges(&self) -> &[ParentEdge] {
        match self {
            Self::Complete(edges) | Self::More { edges, .. } => edges,
        }
    }
    fn edges_mut(&mut self) -> &mut Vec<ParentEdge> {
        match self {
            Self::Complete(edges) | Self::More { edges, .. } => edges,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryNodeSummary {
    pub node_id: String,
    pub kind: HistoryNodeKind,
    pub version_id: String,
    pub sequence: u64,
    pub created_at: String,
    pub branch_id: Option<String>,
    pub path: String,
    pub previous_path: Option<String>,
    pub note: String,
    pub tags: BTreeSet<String>,
    pub size: u64,
    pub content_hash: ContentHash,
    pub content_state: Option<ContentState>,
    pub download_name: String,
    pub storage_name: String,
    pub operation_id: Option<OperationId>,
    pub author: BoundedText,
    pub message: BoundedText,
    pub parents: ParentPreview,
}
impl HistoryNodeSummary {
    /// The same node with `in_window` recomputed against the resident page.
    pub(crate) fn with_window(&self, resident: &BTreeSet<&str>) -> Self {
        let mut node = self.clone();
        for edge in node.parents.edges_mut() {
            edge.in_window = resident.contains(edge.node_id.as_str());
        }
        node
    }
}

#[derive(Clone, Debug)]
pub struct HistoryContext {
    pub path: Option<String>,
    pub note: String,
    pub current_node_id: Option<String>,
    pub selected_branch_id: Option<String>,
    pub content: Option<ContentDispositionSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnchorPlacement {
    pub node_id: String,
    pub in_page: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageDirection {
    Forward,
    Backward,
}
impl PageDirection {
    pub(crate) fn from_backward(backward: bool) -> Self {
        if backward {
            Self::Backward
        } else {
            Self::Forward
        }
    }
    pub(crate) fn range(self, total: usize, admitted: usize) -> std::ops::Range<usize> {
        match self {
            Self::Forward => 0..admitted,
            Self::Backward => total - admitted..total,
        }
    }
}

/// Continuation facts of one selected page, independent of the byte budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Continuation {
    pub direction: PageDirection,
    pub seed: HistoryCursorSeed,
    pub more_before: bool,
    pub more_after: bool,
}

#[derive(Debug)]
pub struct FileHistoryPage {
    pub file_id: String,
    pub repository_id: String,
    pub history_tree_id: String,
    pub history_root: String,
    pub context: HistoryContext,
    pub nodes: Vec<HistoryNodeSummary>,
    pub continuation: Continuation,
    pub anchor: Option<AnchorPlacement>,
    pub limit: usize,
}

#[derive(Debug)]
pub struct NodeParentsPage {
    pub file_id: String,
    pub history_tree_id: String,
    pub history_root: String,
    pub node_id: String,
    pub parents: Vec<String>,
    pub continuation: Continuation,
}

/// Today's branch view without the pending map; `pending_count` replaces it.
#[derive(Clone, Debug)]
pub struct FileBranchSummary {
    pub branch_id: String,
    pub history_tree_id: String,
    pub name: String,
    pub head_commit_id: String,
    pub fork_commit_id: String,
    pub branch_kind: BranchKind,
    pub file_id: String,
    pub file_node_id: Option<String>,
    pub generation: u64,
    pub state: BranchState,
    pub content: ContentDispositionSnapshot,
    pub read_only: bool,
    pub alias_of: Option<String>,
    pub node_id: Option<String>,
    pub current: bool,
    pub pending_count: usize,
}

#[derive(Debug)]
pub struct HistoryBranchesPage {
    pub file_id: String,
    pub history_tree_id: String,
    pub history_root: String,
    pub selected_branch_id: Option<String>,
    pub branches: Vec<FileBranchSummary>,
    pub continuation: Continuation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMergeSummary {
    pub merge_id: String,
    pub sequence: u64,
    pub chosen_node_id: String,
    pub result_node_id: String,
    pub surviving_branch_id: String,
    pub reused_node: bool,
    pub operation_id: OperationId,
    pub input_count: usize,
    pub parent_count: usize,
}

#[derive(Debug)]
pub struct HistoryMergesPage {
    pub file_id: String,
    pub history_tree_id: String,
    pub history_root: String,
    pub merges: Vec<FileMergeSummary>,
    pub continuation: Continuation,
}

/// An index-addressed slice of one subject's relation.
#[derive(Debug)]
pub struct IndexedSlice<T> {
    pub file_id: String,
    pub history_tree_id: String,
    pub history_root: String,
    pub subject_id: String,
    pub offset: usize,
    pub total: usize,
    pub items: Vec<T>,
}
pub type MergeInputsPage = IndexedSlice<(String, String)>;
pub type MergeParentsPage = IndexedSlice<String>;
pub type VersionSourcesPage = IndexedSlice<String>;

#[derive(Debug)]
pub struct NodeFieldSlice {
    pub file_id: String,
    pub history_tree_id: String,
    pub history_root: String,
    pub node_id: String,
    pub field: NodeFieldName,
    pub total_bytes: usize,
    pub offset: usize,
    pub text: String,
    pub complete: bool,
}

/// A bounded read projection: lineage remains in its independently paged
/// relation and is never cloned as a side effect of capturing a version page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionSummary {
    pub history_tree_id: String,
    pub version_id: String,
    pub file_id: String,
    pub creation_primary_slot_id: String,
    pub logical_size: u64,
    pub version_sequence: u64,
    pub content_hash: ContentHash,
    pub checksum_chunk_bytes: u32,
    pub checksum_manifest_hash: String,
    pub creation_name: String,
    pub storage_name: String,
    pub created_at: String,
    pub created_as_full_checkpoint: bool,
    pub source_count: usize,
}
impl From<&storage_contract::FileVersion> for VersionSummary {
    fn from(version: &storage_contract::FileVersion) -> Self {
        let storage_contract::FileVersion {
            history_tree_id,
            version_id,
            file_id,
            creation_primary_slot_id,
            logical_size,
            version_sequence,
            content_hash,
            checksum_chunk_bytes,
            checksum_manifest_hash,
            creation_name,
            storage_name,
            created_at,
            created_as_full_checkpoint,
            source_version_ids,
        } = version;
        Self {
            history_tree_id: history_tree_id.clone(),
            version_id: version_id.clone(),
            file_id: file_id.clone(),
            creation_primary_slot_id: creation_primary_slot_id.clone(),
            logical_size: *logical_size,
            version_sequence: *version_sequence,
            content_hash: *content_hash,
            checksum_chunk_bytes: *checksum_chunk_bytes,
            checksum_manifest_hash: checksum_manifest_hash.clone(),
            creation_name: creation_name.clone(),
            storage_name: storage_name.clone(),
            created_at: created_at.clone(),
            created_as_full_checkpoint: *created_as_full_checkpoint,
            source_count: source_version_ids.len(),
        }
    }
}

#[derive(Debug)]
pub struct VersionPage {
    pub file_id: String,
    pub history_tree_id: String,
    pub history_root: String,
    pub versions: Vec<VersionSummary>,
    pub continuation: Continuation,
}

#[cfg(test)]
#[path = "../../../test/server/unit/backend_file_history.rs"]
mod tests;
