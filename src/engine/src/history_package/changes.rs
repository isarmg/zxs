//! Conservative history dependency closure from automatically captured writes.
//!
//! Ownership is resolved in both states: removals, moves and alias retargeting
//! must invalidate the old owner as well as the new owner. File/directory owner
//! lookup currently scans retained versions/commits once, retaining only dirty
//! IDs. This is not an end-to-end O(delta) claim.
use super::*;
use storage_contract::{MapChanges, Tracked};

const MAX_PENDING_OWNERS: usize = 16_384;

#[derive(Default, Debug)]
pub(super) struct HistoryChanges {
    pub(super) histories: BTreeSet<String>,
    all: bool,
    files: BTreeSet<String>,
    directories: BTreeSet<String>,
}
impl HistoryChanges {
    fn keys<T>(
        &mut self,
        before: &Tracked<T>,
        after: &Tracked<T>,
        mut apply: impl FnMut(&mut Self, &str),
    ) {
        if self.all {
            return;
        }
        match after.changes_since(before) {
            MapChanges::All => self.all = true,
            MapChanges::Keys(keys) => {
                for key in keys.iter() {
                    apply(self, key);
                }
            }
        }
    }
    fn history(&mut self, id: &str) {
        self.histories.insert(id.to_owned());
    }
    fn repository(&mut self, state: &State, id: &str) {
        for history in state
            .histories
            .values()
            .filter(|history| history.repository_id == id)
        {
            self.history(&history.history_tree_id);
        }
    }
    fn file(&mut self, id: &str) {
        if self.files.len() == MAX_PENDING_OWNERS {
            self.all = true;
        } else {
            self.files.insert(id.to_owned());
        }
    }
    fn directory(&mut self, id: &str) {
        if self.directories.len() == MAX_PENDING_OWNERS {
            self.all = true;
        } else {
            self.directories.insert(id.to_owned());
        }
    }
    pub(super) fn between(before: &State, after: &State) -> Self {
        let mut changes = Self::default();
        // This one schema drives exhaustive State matching and dependency
        // application. A new State field cannot compile until classified.
        macro_rules! classify {
            (history { $($history:ident,)* }
             repository { $($repository:ident,)* }
             version { $($version:ident,)* }
             file { $($file:ident,)* }
             directory { $($directory:ident,)* }
             special { $($special:ident,)* }
             independent { $($independent:ident,)* }) => {{
                let State { sequence: _, business_generation: _, topology_generation: _, control_auth: _, events: _,
                    $($history: _,)* $($repository: _,)* $($version: _,)*
                    $($file: _,)* $($directory: _,)* $($special: _,)* $($independent: _,)*
                } = after;
                $(changes.keys(&before.$history, &after.$history, |out, key| {
                    for state in [before, after] {
                        if let Some(value) = state.$history.get(key) { out.history(&value.history_tree_id); }
                    }
                });)*
                $(changes.keys(&before.$repository, &after.$repository, |out, key| {
                    for state in [before, after] {
                        if let Some(value) = state.$repository.get(key) { out.repository(state, &value.repository_id); }
                    }
                });)*
                $(changes.keys(&before.$version, &after.$version, |out, key| {
                    for state in [before, after] {
                        if let Some(version) = state.versions.get(key) { out.history(&version.history_tree_id); }
                    }
                });)*
                $(changes.keys(&before.$file, &after.$file, |out, key| out.file(key));)*
                $(changes.keys(&before.$directory, &after.$directory, |out, key| out.directory(key));)*
            }};
        }
        classify! {
            history { histories, branches, commits, versions, file_path_nodes, file_metadata_nodes, file_branch_merges, merge_links, history_boundaries, }
            repository { repositories, directory_branches, placement_rules, protection_policies, }
            version { placements, content_states, file_node_parents, version_notes, }
            file { file_write_origins, file_locations, file_backup_paths, file_backup_policies, file_selection_generations, file_notes, file_names, }
            directory { directory_tree_selections, directory_selection_generations, }
            special { file_contexts, file_branch_aliases, file_path_notes, file_selections, directory_trees, directory_selections, }
            independent { operations, operation_principals, plans, }
        }
        changes.keys(&before.file_contexts, &after.file_contexts, |out, key| {
            // Context inclusion is repository-prefix based, not branch based.
            if let Some((repository, _)) = key.split_once('/') {
                for state in [before, after] {
                    out.repository(state, repository);
                }
            } else {
                out.all = true;
            }
        });
        changes.keys(
            &before.file_branch_aliases,
            &after.file_branch_aliases,
            |out, key| {
                for state in [before, after] {
                    if let Some(branch) = state.branches.get(key) {
                        out.history(&branch.history_tree_id);
                    }
                }
            },
        );
        changes.keys(
            &before.file_path_notes,
            &after.file_path_notes,
            |out, key| {
                for state in [before, after] {
                    if let Some(node) = state.file_path_nodes.get(key) {
                        out.history(&node.history_tree_id);
                    }
                    if let Some(node) = state.file_metadata_nodes.get(key) {
                        out.history(&node.history_tree_id);
                    }
                }
            },
        );
        changes.keys(
            &before.file_selections,
            &after.file_selections,
            |out, key| {
                for state in [before, after] {
                    if let Some(branch) = state
                        .file_selections
                        .get(key)
                        .and_then(|id| state.branches.get(id))
                    {
                        out.history(&branch.history_tree_id);
                    }
                }
            },
        );
        changes.keys(
            &before.directory_trees,
            &after.directory_trees,
            |out, key| {
                for state in [before, after] {
                    if let Some(branch) = state
                        .directory_trees
                        .get(key)
                        .and_then(|tree| state.directory_branches.get(&tree.directory_branch_id))
                    {
                        out.repository(state, &branch.repository_id);
                    }
                }
            },
        );
        changes.keys(
            &before.directory_selections,
            &after.directory_selections,
            |out, key| {
                for state in [before, after] {
                    if let Some(branch) = state
                        .directory_selections
                        .get(key)
                        .and_then(|id| state.directory_branches.get(id))
                    {
                        out.repository(state, &branch.repository_id);
                    }
                }
            },
        );
        if changes.all {
            changes.histories.extend(
                before
                    .histories
                    .keys()
                    .chain(after.histories.keys())
                    .cloned(),
            );
            return changes;
        }
        for state in [before, after] {
            if !changes.files.is_empty() {
                for version in state.versions.values() {
                    if changes.files.contains(&version.file_id) {
                        changes.histories.insert(version.history_tree_id.clone());
                    }
                }
            }
            if !changes.directories.is_empty() {
                for commit in state.commits.values() {
                    if commit.entries.iter().any(|entry| {
                        entry.kind == "directory" && changes.directories.contains(&entry.entry_id)
                    }) {
                        changes.histories.insert(commit.history_tree_id.clone());
                    }
                }
                for history in state.histories.values() {
                    if changes.directories.contains(&history.repository_id) {
                        changes.histories.insert(history.history_tree_id.clone());
                    }
                }
            }
        }
        changes
    }
}
