//! Only explicitly checked nodes survive replacement of their history page.
use crate::files::history::FileNode;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectedNode {
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) tags: BTreeSet<String>,
}
impl SelectedNode {
    pub(super) fn capture(node: &FileNode) -> Option<Self> {
        node.version_id().map(|version| Self {
            id: node.id().to_owned(),
            version: version.to_owned(),
            tags: node.tags().iter().cloned().collect(),
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    root: Option<String>,
    nodes: BTreeMap<String, SelectedNode>,
}

#[derive(Debug, error_definition::Error)]
pub(crate) enum SelectionError {
    #[error("The history changed. Review all selected nodes before adding another node.")]
    Changed,
    #[error("Every selected node must be resolved before this review can be accepted.")]
    IncompleteReview,
}

impl Selection {
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.nodes.clear();
        self.root = None;
    }

    pub(crate) fn remove(&mut self, id: &str) {
        self.nodes.remove(id);
        if self.nodes.is_empty() {
            self.root = None;
        }
    }

    pub(crate) fn root(&self) -> Option<&str> {
        self.root.as_deref()
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &SelectedNode> {
        self.nodes.values()
    }

    pub(crate) fn get(&self, id: &str) -> Option<&SelectedNode> {
        self.nodes.get(id)
    }

    pub(super) fn insert(&mut self, root: &str, node: SelectedNode) -> Result<(), SelectionError> {
        if self
            .root
            .as_deref()
            .is_some_and(|captured| captured != root)
        {
            return Err(SelectionError::Changed);
        }
        self.root = Some(root.to_owned());
        self.nodes.insert(node.id.clone(), node);
        Ok(())
    }

    /// The caller has resolved every retained identity under the new admitted
    /// root and the user has explicitly accepted that complete review.
    pub(crate) fn accept_review(
        &mut self,
        root: String,
        resolved: Vec<SelectedNode>,
    ) -> Result<(), SelectionError> {
        let count = resolved.len();
        let nodes = resolved
            .into_iter()
            .map(|node| (node.id.clone(), node))
            .collect::<BTreeMap<_, _>>();
        if count != nodes.len() || !nodes.keys().eq(self.nodes.keys()) {
            return Err(SelectionError::IncompleteReview);
        }
        self.nodes = nodes;
        self.root = (!self.nodes.is_empty()).then_some(root);
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/tags/selection_tests.rs"]
mod tests;
