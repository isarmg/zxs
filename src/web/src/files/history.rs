//! One admitted node window and independently replaceable auxiliary windows.
use crate::api;
use json_codec::Value;
use std::sync::Arc;
use storage_contract::history_window::{
    HistoryContentSource, HistoryContextSummary, HistoryField, HistoryMergeSummary,
    HistoryPageCursors, HistoryPageIdentity, HistoryResolvedContext, HistoryWindowPage,
    HistoryWindowRequest,
};

mod ingress;
mod node;
pub(in crate::files) mod reads;
mod view;
mod wire;
pub(crate) use node::{FileNode, NodeKind, Parent, ParentEndpoint, ParentSet};
pub(crate) use reads::Reads;
pub(crate) use view::{auxiliary_view, provenance_view};

#[derive(Debug, error_definition::Error)]
pub enum DecodeError {
    #[error("invalid history window response: {0}")]
    Wire(#[source] json_codec::Error),
    #[error("invalid history window value: {0}")]
    Name(#[source] storage_contract::Error),
    #[error("invalid history window path: {0}")]
    Path(#[source] name_policy::Error),
    #[error("inconsistent history window response: {0}")]
    Relation(&'static str),
}
impl From<DecodeError> for api::Error {
    fn from(source: DecodeError) -> Self {
        Self {
            status: 200,
            code: storage_contract::ErrorCode::InvalidResponse,
            message: "The history window did not match the server contract.".into(),
            retryable: false,
            request_id: None,
            source: Some(std::sync::Arc::new(api::ErrorSource::FileHistory(source))),
        }
    }
}

pub(crate) fn admit(
    value: Value,
    request: &HistoryWindowRequest,
    expected_context: Option<&HistoryResolvedContext>,
) -> Result<HistoryWindowPage, api::Error> {
    ingress::admit(value, request, expected_context).map_err(Into::into)
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

#[derive(Clone, Debug)]
pub(crate) struct FieldFragment {
    pub(crate) node: String,
    pub(crate) field: HistoryField,
    pub(crate) total: u64,
    pub(crate) offset: u64,
    pub(crate) text: String,
    pub(crate) end: bool,
    pub(crate) next: Option<String>,
}

/// Invalidating a basis preserves visible text and editors, but closes writes.
#[derive(Clone, Debug, Default)]
pub(crate) struct FileHistory {
    identity: Option<HistoryPageIdentity>,
    root: Option<String>,
    selected: Option<String>,
    context: Option<HistoryContextSummary>,
    current_id: Option<String>,
    current_version: Option<String>,
    current_inherited: bool,
    current_node: Option<FileNode>,
    nodes: Arc<[FileNode]>,
    page: Option<HistoryPageCursors>,
    branches: Arc<[FileBranch]>,
    branch_page: Option<HistoryPageCursors>,
    current_branch: Option<FileBranch>,
    merges: Arc<[HistoryMergeSummary]>,
    merge_page: Option<HistoryPageCursors>,
    relation: Option<HistoryWindowPage>,
    fragment: Option<FieldFragment>,
    ready: bool,
    window_key: u64,
}
impl FileHistory {
    pub(crate) fn root(&self) -> Option<&str> {
        self.root.as_deref()
    }
    pub(crate) fn loaded(&self) -> bool {
        self.identity.is_some()
    }
    pub(crate) fn ready(&self) -> bool {
        self.ready
    }
    pub(crate) fn invalidate(&mut self) {
        self.ready = false;
    }
    pub(crate) fn nodes(&self) -> &[FileNode] {
        &self.nodes
    }
    pub(crate) fn node(&self, id: &str) -> Option<&FileNode> {
        self.nodes
            .iter()
            .find(|node| node.id() == id)
            .or_else(|| self.current_node.as_ref().filter(|node| node.id() == id))
    }
    pub(crate) fn body_note(&self, version: &str) -> Option<&str> {
        self.nodes
            .iter()
            .chain(self.current_node.iter())
            .find(|node| node.version_id() == Some(version) && node.body_note().is_some())
            .and_then(FileNode::body_note)
    }
    pub(crate) fn branches(&self) -> &[FileBranch] {
        &self.branches
    }
    pub(crate) fn current_branch(&self) -> Option<&FileBranch> {
        self.current_branch.as_ref()
    }
    pub(crate) fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }
    pub(crate) fn file_note(&self) -> Option<&str> {
        self.context.as_ref().map(|c| c.note.as_str())
    }
    pub(crate) fn current_node_id(&self) -> Option<&str> {
        self.current_id.as_deref()
    }
    pub(crate) fn current_version_id(&self) -> Option<&str> {
        self.current_version.as_deref()
    }
    pub(crate) fn current_inherited(&self) -> bool {
        self.current_inherited
    }
    pub(crate) fn window_key(&self) -> u64 {
        self.window_key
    }
    pub(crate) fn action_max_entries(&self) -> Option<usize> {
        // The 1 MiB action envelope gives a much smaller count bound on WASM32.
        Some(
            self.identity
                .as_ref()?
                .action_max_entries
                .min(usize::MAX as u64) as usize,
        )
    }
    pub(crate) fn identity(&self) -> Option<&HistoryPageIdentity> {
        self.identity.as_ref()
    }
    pub(crate) fn next_cursor(&self) -> Option<&str> {
        self.page.as_ref()?.next_cursor.as_ref().map(|c| c.as_str())
    }
    pub(crate) fn previous_cursor(&self) -> Option<&str> {
        self.page
            .as_ref()?
            .previous_cursor
            .as_ref()
            .map(|c| c.as_str())
    }
    pub(crate) fn next_branch_cursor(&self) -> Option<&str> {
        self.branch_page
            .as_ref()?
            .next_cursor
            .as_ref()
            .map(|c| c.as_str())
    }
    pub(crate) fn previous_branch_cursor(&self) -> Option<&str> {
        self.branch_page
            .as_ref()?
            .previous_cursor
            .as_ref()
            .map(|c| c.as_str())
    }
    pub(crate) fn merges(&self) -> &[HistoryMergeSummary] {
        &self.merges
    }
    pub(crate) fn merge_cursors(&self) -> Option<&HistoryPageCursors> {
        self.merge_page.as_ref()
    }
    pub(crate) fn relation(&self) -> Option<&HistoryWindowPage> {
        self.relation.as_ref()
    }
    pub(crate) fn fragment(&self) -> Option<&FieldFragment> {
        self.fragment.as_ref()
    }
    pub(crate) fn close_relation(&mut self) {
        self.relation = None;
    }
    pub(crate) fn close_fragment(&mut self) {
        self.fragment = None;
    }

    pub(crate) fn install_nodes(
        &mut self,
        page: HistoryWindowPage,
        entry: &super::page::Entry,
        replace: bool,
    ) -> Result<(), DecodeError> {
        let HistoryWindowPage::Nodes {
            identity,
            context,
            nodes,
            edges,
            page,
        } = page
        else {
            return Err(DecodeError::Relation(
                "node loader received another collection",
            ));
        };
        let root = identity.history_root.to_string();
        let same_root = self.root.as_deref() == Some(root.as_str());
        let nodes = wire::nodes(nodes, edges);
        if entry.backup()
            && nodes.iter().any(|node| {
                Some(node.id()) == entry.node_id() && node.version_id() != entry.version_id()
            })
        {
            return Err(DecodeError::Relation(
                "backup node differs from its stored body version",
            ));
        }
        let same_structure = self.nodes.len() == nodes.len()
            && self.nodes.iter().zip(&nodes).all(|(old, new)| {
                old.id() == new.id()
                    && old.parents() == new.parents()
                    && old.parent_set() == new.parent_set()
            });
        if replace || !same_structure || !self.loaded() {
            self.window_key += 1;
        }
        if !same_root {
            self.branches = Default::default();
            self.branch_page = None;
            self.current_branch = None;
            self.merges = Default::default();
            self.merge_page = None;
            self.relation = None;
            self.fragment = None;
            self.current_node = None;
        }
        self.current_id = if entry.backup() {
            entry.node_id().map(str::to_owned)
        } else {
            identity
                .context
                .current_node_id
                .as_ref()
                .map(ToString::to_string)
        };
        self.current_version = if entry.backup() {
            entry.version_id().map(str::to_owned)
        } else {
            context
                .content
                .as_ref()
                .and_then(|c| c.version_id.as_ref())
                .map(ToString::to_string)
        };
        self.current_inherited = if entry.backup() {
            entry.inherited_content()
        } else {
            context
                .content
                .as_ref()
                .is_some_and(|c| matches!(c.source, HistoryContentSource::Inherited { .. }))
        };
        if let Some(node) = nodes
            .iter()
            .find(|node| Some(node.id()) == self.current_id.as_deref())
        {
            self.current_node = Some(node.clone());
        }
        self.selected = identity
            .context
            .selected_branch_id
            .as_ref()
            .map(ToString::to_string);
        self.identity = Some(identity);
        self.root = Some(root);
        self.context = Some(context);
        self.nodes = nodes.into();
        self.page = Some(page);
        self.ready = true;
        Ok(())
    }

    pub(crate) fn install_auxiliary(&mut self, page: HistoryWindowPage) -> Result<(), DecodeError> {
        match page {
            HistoryWindowPage::Branches {
                identity: _,
                branches,
                page,
            } => {
                let branches = branches.into_iter().map(wire::branch).collect::<Vec<_>>();
                if let Some(branch) = branches.iter().find(|b| Some(b.id()) == self.selected()) {
                    self.current_branch = Some(branch.clone());
                }
                self.branches = branches.into();
                self.branch_page = Some(page);
            }
            HistoryWindowPage::Merges {
                identity: _,
                merges,
                page,
            } => {
                self.merges = merges.into();
                self.merge_page = Some(page);
            }
            HistoryWindowPage::NodeField {
                identity,
                field,
                total_bytes,
                byte_offset,
                text,
                end_of_field,
                next_cursor,
            } => {
                let storage_contract::history_window::HistoryCollection::NodeField {
                    node_id, ..
                } = identity.collection
                else {
                    return Err(DecodeError::Relation(
                        "field fragment has another collection",
                    ));
                };
                self.fragment = Some(FieldFragment {
                    node: node_id.to_string(),
                    field,
                    total: total_bytes,
                    offset: byte_offset,
                    text,
                    end: end_of_field,
                    next: next_cursor.map(|c| c.as_str().to_owned()),
                });
            }
            page @ (HistoryWindowPage::NodeParents { .. }
            | HistoryWindowPage::MergeInputs { .. }
            | HistoryWindowPage::MergeParents { .. }
            | HistoryWindowPage::VersionSources { .. }) => {
                self.relation = Some(page);
            }
            HistoryWindowPage::Nodes { .. } | HistoryWindowPage::Versions { .. } => {
                return Err(DecodeError::Relation(
                    "auxiliary loader received another collection",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/history/tests.rs"]
pub(crate) mod tests;
