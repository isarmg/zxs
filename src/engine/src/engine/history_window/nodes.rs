//! Borrowed node facts. Cold scans spend CPU, never a history-sized work list.
use super::*;
use crate::{Commit, FileMetadataNode, FilePathNode, FileVersion};
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub(super) struct Scope<'a> {
    pub(super) file: &'a str,
    pub(super) history: &'a str,
}

#[derive(Clone, Copy)]
pub(super) enum Node<'a> {
    Content(&'a FileVersion),
    Path(&'a FilePathNode),
    Metadata(&'a FileMetadataNode),
}

impl<'a> Node<'a> {
    pub(super) fn id(self) -> &'a str {
        match self {
            Self::Content(node) => &node.version_id,
            Self::Path(node) => &node.node_id,
            Self::Metadata(node) => &node.node_id,
        }
    }

    pub(super) fn version(self, state: &'a State) -> &'a FileVersion {
        match self {
            Self::Content(version) => version,
            Self::Path(node) => &state.versions[&node.version_id],
            Self::Metadata(node) => &state.versions[&node.version_id],
        }
    }

    pub(super) fn sequence(self, commit: Option<&Commit>) -> u64 {
        match self {
            Self::Content(version) => {
                commit.map_or(version.version_sequence, |commit| commit.sequence)
            }
            Self::Path(node) => node.sequence,
            Self::Metadata(node) => node.sequence,
        }
    }
}

pub(super) fn node<'a>(state: &'a State, scope: Scope<'_>, id: &str) -> Option<Node<'a>> {
    state
        .versions
        .get(id)
        .filter(|node| node.file_id == scope.file && node.history_tree_id == scope.history)
        .map(Node::Content)
        .or_else(|| {
            state
                .file_path_nodes
                .get(id)
                .filter(|node| node.file_id == scope.file && node.history_tree_id == scope.history)
                .map(Node::Path)
        })
        .or_else(|| {
            state
                .file_metadata_nodes
                .get(id)
                .filter(|node| node.file_id == scope.file && node.history_tree_id == scope.history)
                .map(Node::Metadata)
        })
}

pub(super) fn all_nodes<'a>(
    state: &'a State,
    scope: Scope<'a>,
    cancel: &'a HistoryWindowCancellation,
) -> impl Iterator<Item = Result<Node<'a>>> {
    state
        .versions
        .values()
        .map(Node::Content)
        .chain(state.file_path_nodes.values().map(Node::Path))
        .chain(state.file_metadata_nodes.values().map(Node::Metadata))
        .filter_map(move |candidate| {
            if let Err(error) = cancel.checkpoint() {
                return Some(Err(error));
            }
            let (file, history) = match candidate {
                Node::Content(node) => (&node.file_id, &node.history_tree_id),
                Node::Path(node) => (&node.file_id, &node.history_tree_id),
                Node::Metadata(node) => (&node.file_id, &node.history_tree_id),
            };
            (file == scope.file && history == scope.history).then_some(Ok(candidate))
        })
}

pub(super) fn base_parents<'a>(state: &'a State, id: &str) -> &'a [String] {
    if let Some(node) = state.file_metadata_nodes.get(id) {
        &node.parent_node_ids
    } else if let Some(node) = state.file_path_nodes.get(id) {
        &node.parent_node_ids
    } else if let Some(version) = state.versions.get(id) {
        state
            .file_node_parents
            .get(id)
            .unwrap_or(&version.source_version_ids)
    } else {
        &[]
    }
}

pub(super) fn first_commit<'a>(
    state: &'a State,
    scope: Scope<'_>,
    version: &str,
    cancel: &HistoryWindowCancellation,
) -> Result<Option<&'a Commit>> {
    let mut first = None::<&Commit>;
    for commit in state.commits.values() {
        cancel.checkpoint()?;
        if commit.history_tree_id != scope.history {
            continue;
        }
        for entry in &commit.entries {
            cancel.checkpoint()?;
            if entry.file_id == scope.file
                && entry.version_id == version
                && first.is_none_or(|first| commit.sequence < first.sequence)
            {
                first = Some(commit);
            }
        }
    }
    Ok(first)
}

fn first_parent<'a>(state: &'a State, scope: Scope<'_>, id: &str) -> Option<&'a str> {
    base_parents(state, id)
        .first()
        .filter(|parent| node(state, scope, parent).is_some())
        .map(String::as_str)
}

/// Preserve first-parent tag inheritance without a growing visited/cache map.
/// The admitted node count bounds a walk even for a malformed cyclic fixture.
pub(super) fn tags<'a>(
    state: &'a State,
    scope: Scope<'_>,
    id: &'a str,
    cancel: &HistoryWindowCancellation,
) -> Result<Option<&'a BTreeSet<String>>> {
    let mut current = id;
    let maximum = state
        .versions
        .len()
        .saturating_add(state.file_path_nodes.len())
        .saturating_add(state.file_metadata_nodes.len());
    for _ in 0..maximum {
        cancel.checkpoint()?;
        if let Some(metadata) = state
            .file_metadata_nodes
            .get(current)
            .filter(|node| node.file_id == scope.file && node.history_tree_id == scope.history)
        {
            return Ok(Some(&metadata.tags));
        }
        let Some(parent) = first_parent(state, scope, current) else {
            return Ok(None);
        };
        current = parent;
    }
    Ok(None)
}

/// The same canonical source-chain path as the full projection. Only the final
/// bounded path is copied by the page builder; ancestors remain borrowed.
pub(super) fn path<'a>(
    state: &'a State,
    scope: Scope<'_>,
    selected: Node<'a>,
    cancel: &HistoryWindowCancellation,
) -> Result<&'a str> {
    let mut current = selected;
    let maximum = state
        .versions
        .len()
        .saturating_add(state.file_path_nodes.len())
        .saturating_add(state.file_metadata_nodes.len());
    for _ in 0..maximum {
        cancel.checkpoint()?;
        match current {
            Node::Path(node) => return Ok(&node.path),
            Node::Content(version) => {
                if let Some(commit) = first_commit(state, scope, &version.version_id, cancel)? {
                    for entry in &commit.entries {
                        cancel.checkpoint()?;
                        if entry.file_id == scope.file && entry.version_id == version.version_id {
                            return Ok(entry.path());
                        }
                    }
                }
                return Ok(&version.creation_name);
            }
            Node::Metadata(metadata) => {
                let source = if metadata.kind == crate::FileMetadataKind::Merge {
                    let mut chosen = None;
                    for merge in state.file_branch_merges.values() {
                        cancel.checkpoint()?;
                        if merge.file_id == scope.file
                            && merge.history_tree_id == scope.history
                            && !merge.reused_node
                            && merge.result_node_id == metadata.node_id
                        {
                            chosen = Some(merge.chosen_node_id.as_str());
                        }
                    }
                    chosen
                } else {
                    metadata.parent_node_ids.first().map(String::as_str)
                };
                match source.and_then(|source| node(state, scope, source)) {
                    Some(source) => current = source,
                    None => return Ok(&state.versions[&metadata.version_id].creation_name),
                }
            }
        }
    }
    Err(Error::new(
        storage_contract::ErrorCode::StateInvalid,
        "history source chain is cyclic",
    ))
}

pub(super) fn visit_parents<'a>(
    state: &'a State,
    scope: Scope<'_>,
    id: &str,
    cancel: &HistoryWindowCancellation,
    mut visit: impl FnMut(&'a str) -> Result<()>,
) -> Result<()> {
    for parent in base_parents(state, id) {
        cancel.checkpoint()?;
        if node(state, scope, parent).is_some() {
            visit(parent)?;
        }
    }
    for merge in state.file_branch_merges.values() {
        cancel.checkpoint()?;
        if merge.file_id == scope.file
            && merge.history_tree_id == scope.history
            && merge.result_node_id == id
        {
            for parent in &merge.parent_node_ids {
                cancel.checkpoint()?;
                if parent != id && node(state, scope, parent).is_some() {
                    visit(parent)?;
                }
            }
        }
    }
    Ok(())
}
