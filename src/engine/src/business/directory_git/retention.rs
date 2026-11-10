//! Body retention for explicit Git HEADs, staged owned files and pinned children.
use super::ChildBoundaries;
use crate::{DirectoryTree, Entry, NamespaceEntryKind, State};
use std::collections::{BTreeMap, BTreeSet};

/// Resolve boundaries by stable identity in this snapshot, not by their later
/// working paths. Each file checks its ancestor keys instead of scanning every
/// independent child. The staged child's working body is not part of this index.
pub(crate) fn staged_owned_files<'a>(
    tree: &'a DirectoryTree,
    boundaries: &BTreeMap<String, String>,
) -> impl Iterator<Item = &'a Entry> {
    let children = ChildBoundaries::new(&tree.entries, |id| boundaries.contains_key(id));
    tree.entries.iter().filter(move |entry| {
        entry.kind == NamespaceEntryKind::File && !children.contains(entry.lookup_key())
    })
}

/// State admission proves that Git HEADs and explicit child references name
/// committed trees and that each index tree belongs to its directory branch.
/// Historical ancestors are not retention roots; only the current persisted
/// references pin their bodies. Keep only the caller's requested version IDs.
pub(crate) fn protected_git_versions<'a>(
    state: &'a State,
    selected: &BTreeSet<&String>,
) -> BTreeSet<&'a str> {
    let mut protected = BTreeSet::new();
    let mut committed = BTreeSet::new();
    for git in state.directory_git_repositories.values() {
        for index in git.branches.values() {
            committed.extend(index.head_tree_id.iter());
            committed.extend(
                index
                    .index_children
                    .values()
                    .filter_map(|reference| reference.tree_id.as_ref()),
            );
            let tree = &state.directory_trees[&index.index_tree_id];
            protected.extend(
                staged_owned_files(tree, &index.index_boundaries)
                    .filter(|entry| selected.contains(&entry.version_id))
                    .map(|entry| entry.version_id.as_str()),
            );
        }
    }
    for tree_id in committed {
        protected.extend(
            state.directory_trees[tree_id]
                .entries
                .iter()
                .filter(|entry| {
                    entry.kind == NamespaceEntryKind::File && selected.contains(&entry.version_id)
                })
                .map(|entry| entry.version_id.as_str()),
        );
    }
    protected
}
