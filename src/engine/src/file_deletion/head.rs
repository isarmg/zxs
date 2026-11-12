//! Borrowed identities of current branch HEADs, separate from their real bodies.
use crate::{FileMetadataKind, FileMetadataNode, FilePathNode, FileVersion, State};

/// Existing public file node kinds. This is an observation, not a persisted tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileDeletionNodeKind {
    Content,
    Rename,
    Merge,
}

#[derive(Debug)]
enum Node<'a> {
    Content(&'a FileVersion),
    Rename(&'a FilePathNode),
    Metadata(&'a FileMetadataNode),
}

/// A current HEAD and the existing body it describes, borrowed from one Engine.
/// Metadata or rename HEAD IDs remain distinct even when all bodies are equal.
/// This type cannot describe a bodyless deletion node and is not a retention lease.
#[derive(Debug)]
pub struct FileDeletionHead<'a> {
    node: Node<'a>,
    body: &'a FileVersion,
}

impl<'a> FileDeletionHead<'a> {
    /// Called only after the existing branch_head resolver has admitted this node.
    pub(super) fn retained(state: &'a State, node_id: &str) -> Self {
        let node = if let Some(version) = state.versions.get(node_id) {
            Node::Content(version)
        } else if let Some(path) = state.file_path_nodes.get(node_id) {
            Node::Rename(path)
        } else {
            Node::Metadata(
                state
                    .file_metadata_nodes
                    .get(node_id)
                    .expect("invariant: an admitted branch head is an existing file node"),
            )
        };
        let body = match &node {
            Node::Content(version) => *version,
            Node::Rename(path) => state
                .versions
                .get(&path.version_id)
                .expect("invariant: an admitted rename head references an existing body"),
            Node::Metadata(metadata) => state
                .versions
                .get(&metadata.version_id)
                .expect("invariant: an admitted metadata head references an existing body"),
        };
        Self { node, body }
    }

    pub fn node_id(&self) -> &'a str {
        match &self.node {
            Node::Content(version) => &version.version_id,
            Node::Rename(path) => &path.node_id,
            Node::Metadata(metadata) => &metadata.node_id,
        }
    }

    pub fn kind(&self) -> FileDeletionNodeKind {
        match &self.node {
            Node::Content(_) => FileDeletionNodeKind::Content,
            Node::Rename(_) => FileDeletionNodeKind::Rename,
            Node::Metadata(metadata) => match metadata.kind {
                FileMetadataKind::Merge => FileDeletionNodeKind::Merge,
            },
        }
    }

    /// Existing body identity and metadata only; no body bytes are read or pinned.
    pub fn body(&self) -> &'a FileVersion {
        self.body
    }

    pub(super) fn history_tree_id(&self) -> &'a str {
        match &self.node {
            Node::Content(version) => &version.history_tree_id,
            Node::Rename(path) => &path.history_tree_id,
            Node::Metadata(metadata) => &metadata.history_tree_id,
        }
    }
}
