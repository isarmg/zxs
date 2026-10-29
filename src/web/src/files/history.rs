//! Bounded file-history pages: every response is admitted once at the wire
//! boundary into private, typed records. Nothing here retains raw JSON.
use crate::api;
use std::collections::BTreeSet;

pub(crate) mod state;
mod wire;

#[derive(Debug, error_definition::Error)]
pub enum DecodeError {
    #[error("invalid file-history response: {0}")]
    Wire(#[source] json_codec::Error),
    #[error("invalid file-history branch name: {0}")]
    Name(#[source] storage_contract::Error),
    #[error("inconsistent file-history response: {0}")]
    Relation(&'static str),
}
impl From<DecodeError> for api::Error {
    fn from(source: DecodeError) -> Self {
        Self {
            status: 200,
            code: storage_contract::ErrorCode::InvalidResponse,
            message: "The file history did not match the server contract.".into(),
            retryable: false,
            request_id: None,
            source: Some(std::sync::Arc::new(api::ErrorSource::FileHistory(source))),
        }
    }
}

/// The exact `history-manifest` hash the server bound this page to. Every
/// auxiliary read and continuation carries it; a mismatch is `HISTORY_CHANGED`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct HistoryRoot(String);
impl HistoryRoot {
    pub(crate) fn parse(value: &str) -> Result<Self, DecodeError> {
        if value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(Self(value.to_owned()))
        } else {
            Err(DecodeError::Relation(
                "history_root must be 64 lowercase hexadecimal characters",
            ))
        }
    }
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identity of one admitted page: file, history, root and the context the
/// server resolved for it. Continuations never silently change any of these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HistoryBinding {
    pub(crate) file_id: String,
    pub(crate) history_tree_id: String,
    pub(crate) root: HistoryRoot,
    pub(crate) repository_id: String,
    pub(crate) path: Option<String>,
}

/// Content disposition of the current entry, present only when the file has
/// a current namespace entry in the resolved context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ContentDisposition {
    pub(crate) version_id: Option<String>,
    pub(crate) materialized: bool,
    pub(crate) inherited: bool,
    pub(crate) own_version_id: Option<String>,
    pub(crate) source_version_id: Option<String>,
    pub(crate) content_state: String,
    pub(crate) visible_content_state: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HistoryContext {
    pub(crate) note: String,
    pub(crate) current_node_id: Option<String>,
    pub(crate) selected_branch_id: Option<String>,
    pub(crate) content: Option<ContentDisposition>,
}
impl HistoryContext {
    pub(crate) fn inherited(&self) -> bool {
        self.content.as_ref().is_some_and(|content| {
            content.inherited
                && content
                    .version_id
                    .as_deref()
                    .is_some_and(|id| !id.is_empty())
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeKind {
    Content,
    Rename,
    Tags,
    Merge,
}
impl NodeKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Rename => "rename",
            Self::Tags => "tags",
            Self::Merge => "merge",
        }
    }
}

/// A text without a persisted write limit. The server inlines it only within
/// `MAX_INLINE_TEXT_BYTES`; longer values are read through the field route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BoundedText {
    pub(crate) bytes: usize,
    pub(crate) inline: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ParentStatus {
    Complete,
    More { cursor: String },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParentEdge {
    pub(crate) node_id: String,
    pub(crate) in_window: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParentPreview {
    pub(crate) status: ParentStatus,
    pub(crate) edges: Vec<ParentEdge>,
}
impl ParentPreview {
    pub(crate) fn more(&self) -> bool {
        matches!(self.status, ParentStatus::More { .. })
    }
    pub(crate) fn in_window(&self) -> impl Iterator<Item = &str> {
        self.edges
            .iter()
            .filter(|edge| edge.in_window)
            .map(|edge| edge.node_id.as_str())
    }
    pub(crate) fn outside_window(&self) -> impl Iterator<Item = &str> {
        self.edges
            .iter()
            .filter(|edge| !edge.in_window)
            .map(|edge| edge.node_id.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HistoryNodeSummary {
    pub(crate) node_id: String,
    pub(crate) kind: NodeKind,
    pub(crate) version_id: String,
    pub(crate) sequence: u64,
    pub(crate) created_at: String,
    pub(crate) branch_id: Option<String>,
    pub(crate) path: String,
    pub(crate) previous_path: Option<String>,
    pub(crate) note: String,
    pub(crate) tags: BTreeSet<String>,
    pub(crate) size: u64,
    pub(crate) content_hash: String,
    pub(crate) content_state: String,
    pub(crate) download_name: String,
    pub(crate) storage_name: String,
    pub(crate) operation_id: Option<String>,
    pub(crate) author: BoundedText,
    pub(crate) message: BoundedText,
    pub(crate) parents: ParentPreview,
    pub(crate) current: bool,
}

/// Identity of a resident page: the same root and first node key describe the
/// same node set, so editors anchored to a node may survive a refresh.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct WindowIdentity {
    pub(crate) root: HistoryRoot,
    pub(crate) first_node: Option<String>,
}

/// The one resident main page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HistoryWindow {
    pub(crate) binding: HistoryBinding,
    pub(crate) context: HistoryContext,
    pub(crate) nodes: Vec<HistoryNodeSummary>,
    pub(crate) previous_cursor: Option<String>,
    pub(crate) next_cursor: Option<String>,
    pub(crate) anchor: Option<String>,
}
impl HistoryWindow {
    pub(crate) fn parse(value: json_codec::Value, file: &str) -> Result<Self, api::Error> {
        wire::window(value, file).map_err(Into::into)
    }
    pub(crate) fn root(&self) -> &HistoryRoot {
        &self.binding.root
    }
    pub(crate) fn identity(&self) -> WindowIdentity {
        WindowIdentity {
            root: self.binding.root.clone(),
            first_node: self.nodes.first().map(|node| node.node_id.clone()),
        }
    }
    /// Linear page lookup for one-off reads; shared consumers use `HistoryIndex`.
    pub(crate) fn node(&self, id: &str) -> Option<&HistoryNodeSummary> {
        self.nodes.iter().find(|node| node.node_id == id)
    }
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.node(id).is_some()
    }
    pub(crate) fn is_first_page(&self) -> bool {
        self.previous_cursor.is_none()
    }
    /// Only a complete first page without continuation proves an empty history.
    pub(crate) fn has_no_history(&self) -> bool {
        self.nodes.is_empty() && self.previous_cursor.is_none() && self.next_cursor.is_none()
    }
    /// Backup entries stay pinned to their stored node even when the primary
    /// alias has advanced; the page keeps every node, only `current` moves.
    pub(crate) fn for_entry(mut self, entry: &super::page::Entry) -> Self {
        if !entry.backup() {
            return self;
        }
        let node = entry
            .node_id()
            .expect("invariant: backup file has a node identity")
            .to_owned();
        let version = entry
            .version_id()
            .expect("invariant: backup file has its stored version")
            .to_owned();
        if let Some(content) = self.context.content.as_mut() {
            content.version_id = Some(version);
            content.inherited = entry.inherited();
            content.materialized = entry.materialized();
        }
        self.context.current_node_id = Some(node);
        self.mark_current();
        self
    }
    fn mark_current(&mut self) {
        let current = self.context.current_node_id.as_deref();
        for node in &mut self.nodes {
            node.current = current == Some(node.node_id.as_str());
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileBranch {
    id: String,
    name: String,
    head: Option<String>,
    pending: bool,
    writable: bool,
}
impl FileBranch {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn head(&self) -> Option<&str> {
        self.head.as_deref()
    }
    pub(crate) fn pending(&self) -> bool {
        self.pending
    }
    pub(crate) fn writable(&self) -> bool {
        self.writable
    }
}

/// One branch-selector page bound to the resident root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BranchPage {
    root: HistoryRoot,
    branches: Vec<FileBranch>,
    pub(crate) previous_cursor: Option<String>,
    pub(crate) next_cursor: Option<String>,
}
impl BranchPage {
    pub(crate) fn parse(
        value: json_codec::Value,
        binding: &HistoryBinding,
    ) -> Result<Self, api::Error> {
        wire::branch_page(value, binding).map_err(Into::into)
    }
    pub(crate) fn root(&self) -> &HistoryRoot {
        &self.root
    }
    pub(crate) fn branches(&self) -> &[FileBranch] {
        &self.branches
    }
    pub(crate) fn branch(&self, id: &str) -> Option<&FileBranch> {
        self.branches.iter().find(|branch| branch.id == id)
    }
    pub(crate) fn writable(&self) -> impl Iterator<Item = &FileBranch> {
        self.branches.iter().filter(|branch| branch.writable)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MergeSummary {
    pub(crate) merge_id: String,
    pub(crate) sequence: u64,
    pub(crate) chosen_node_id: String,
    pub(crate) result_node_id: String,
    pub(crate) surviving_branch_id: String,
    pub(crate) reused_node: bool,
    pub(crate) operation_id: String,
    pub(crate) input_count: usize,
    pub(crate) parent_count: usize,
}

/// One provenance page, ordered `(sequence DESC, merge_id ASC)` by the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MergePage {
    root: HistoryRoot,
    pub(crate) merges: Vec<MergeSummary>,
    pub(crate) previous_cursor: Option<String>,
    pub(crate) next_cursor: Option<String>,
}
impl MergePage {
    pub(crate) fn parse(
        value: json_codec::Value,
        binding: &HistoryBinding,
    ) -> Result<Self, api::Error> {
        wire::merge_page(value, binding).map_err(Into::into)
    }
    pub(crate) fn root(&self) -> &HistoryRoot {
        &self.root
    }
}

/// One window of a node's complete graph parents, parent IDs ascending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParentsPage {
    pub(crate) node_id: String,
    pub(crate) parents: Vec<String>,
    pub(crate) previous_cursor: Option<String>,
    pub(crate) next_cursor: Option<String>,
}
impl ParentsPage {
    pub(crate) fn parse(
        value: json_codec::Value,
        binding: &HistoryBinding,
        node: &str,
    ) -> Result<Self, api::Error> {
        wire::parents_page(value, binding, node).map_err(Into::into)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MergeInput {
    pub(crate) index: usize,
    pub(crate) branch_id: String,
    pub(crate) node_id: String,
}

/// Merge inputs by original index; pairs are never split across pages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MergeInputsPage {
    pub(crate) merge_id: String,
    pub(crate) offset: usize,
    pub(crate) total: usize,
    pub(crate) inputs: Vec<MergeInput>,
    pub(crate) next_offset: Option<usize>,
}
impl MergeInputsPage {
    pub(crate) fn parse(
        value: json_codec::Value,
        binding: &HistoryBinding,
        merge: &str,
    ) -> Result<Self, api::Error> {
        wire::merge_inputs(value, binding, merge).map_err(Into::into)
    }
    pub(crate) fn previous_offset(&self) -> Option<usize> {
        (self.offset > 0).then(|| self.offset.saturating_sub(self.inputs.len().max(1)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeField {
    Message,
    Author,
}
impl NodeField {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Author => "author",
        }
    }
}

/// One UTF-8 fragment of an externalized node text. `complete` is true only
/// when this fragment ends exactly at the field's last byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FieldSlice {
    pub(crate) node_id: String,
    pub(crate) field: NodeField,
    pub(crate) total_bytes: usize,
    pub(crate) offset: usize,
    pub(crate) text: String,
    pub(crate) complete: bool,
    pub(crate) next_offset: Option<usize>,
}
impl FieldSlice {
    pub(crate) fn parse(
        value: json_codec::Value,
        binding: &HistoryBinding,
        node: &str,
        field: NodeField,
    ) -> Result<Self, api::Error> {
        wire::field_slice(value, binding, node, field).map_err(Into::into)
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/history/tests.rs"]
pub(crate) mod tests;
