//! Preserve the exact hidden subtree and its identity while D is active.
//! Empty directories are fenced too; the file-D guard alone cannot cover them.
use super::*;
use std::collections::BTreeSet;
use storage_contract::{MapChanges, TrackedMap};

fn changed<T>(before: &TrackedMap<T>, after: &TrackedMap<T>) -> Vec<String> {
    match after.changes_since(before) {
        MapChanges::Keys(keys) => keys.iter().cloned().collect(),
        MapChanges::All => before
            .keys()
            .chain(after.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    }
}

fn protected<'a>(entries: &'a [Entry], roots: &BTreeSet<&str>, whole: bool) -> Vec<&'a Entry> {
    if whole {
        return entries.iter().collect();
    }
    let mut paths = BTreeSet::new();
    for entry in entries
        .iter()
        .filter(|entry| roots.contains(entry.entry_id.as_str()))
    {
        paths.insert(entry.lookup_key());
    }
    if paths.is_empty() {
        return Vec::new();
    }
    let mut ancestors = BTreeSet::new();
    for root in &paths {
        let mut key = *root;
        while let Some((parent, _)) = key.rsplit_once('/') {
            ancestors.insert(parent);
            key = parent;
        }
    }
    entries
        .iter()
        .filter(|entry| {
            if ancestors.contains(entry.lookup_key()) {
                return true;
            }
            let mut key = entry.lookup_key();
            loop {
                if paths.contains(key) {
                    return true;
                }
                let Some((parent, _)) = key.rsplit_once('/') else {
                    return false;
                };
                key = parent;
            }
        })
        .collect()
}

fn branch_entries<'a>(state: &'a State, id: &str) -> &'a [Entry] {
    state
        .branches
        .get(id)
        .and_then(|branch| state.commits.get(&branch.head_commit_id))
        .map_or(&[], |commit| &commit.entries)
}

struct Selected<'a> {
    path: &'a str,
    entries: &'a [Entry],
    contexts: &'a BTreeMap<String, crate::FileContext>,
}

fn selected<'a>(state: &'a State, directory: &str) -> Option<Selected<'a>> {
    let branch = state
        .directory_selections
        .get(directory)
        .and_then(|id| state.directory_branches.get(id))?;
    match state.directory_tree_selections.get(directory) {
        Some(tree) => state.directory_trees.get(tree).map(|tree| Selected {
            path: &tree.path,
            entries: &tree.entries,
            contexts: &tree.file_contexts,
        }),
        None => Some(Selected {
            path: &branch.path,
            entries: &branch.entries,
            contexts: &branch.file_contexts,
        }),
    }
}

pub(crate) fn verify(previous: &State, next: &State) -> Result<()> {
    let nodes = previous
        .directory_git_repositories
        .values()
        .flat_map(|git| git.deletions.values())
        .collect::<Vec<_>>();
    if nodes.is_empty() {
        return Ok(());
    }
    let roots = nodes
        .iter()
        .map(|node| node.directory_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut directories = BTreeSet::new();
    for node in &nodes {
        let owner = previous
            .directory_git_repositories
            .get(&node.owner_directory_id)
            .ok_or_else(invalid)?;
        let after = next
            .directory_git_repositories
            .get(&node.owner_directory_id)
            .ok_or_else(frozen)?;
        if owner.current_branch_id != after.current_branch_id
            || previous.directory_selections.get(&node.owner_directory_id)
                != next.directory_selections.get(&node.owner_directory_id)
        {
            return Err(frozen());
        }
        let original = previous
            .commits
            .get(&node.namespace_head)
            .ok_or_else(invalid)?;
        directories.extend(
            source::subtree(&original.entries, node.path.key())
                .into_iter()
                .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
                .map(|entry| entry.entry_id.as_str()),
        );
    }
    for id in changed(&previous.branches, &next.branches) {
        if protected(branch_entries(previous, &id), &roots, false)
            != protected(branch_entries(next, &id), &roots, false)
        {
            return Err(frozen());
        }
    }
    for id in changed(&previous.directory_branches, &next.directory_branches) {
        let before = previous.directory_branches.get(&id);
        let after = next.directory_branches.get(&id);
        let whole = before
            .into_iter()
            .chain(after)
            .any(|branch| directories.contains(branch.directory_id.as_str()));
        let old = protected(before.map_or(&[], |branch| &branch.entries), &roots, whole);
        let new = protected(after.map_or(&[], |branch| &branch.entries), &roots, whole);
        if old != new {
            return Err(frozen());
        }
        if whole
            && before.map(|branch| (&branch.directory_id, &branch.path))
                != after.map(|branch| (&branch.directory_id, &branch.path))
        {
            return Err(frozen());
        }
        for entry in old
            .into_iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
        {
            if before.and_then(|branch| branch.file_contexts.get(entry.lookup_key()))
                != after.and_then(|branch| branch.file_contexts.get(entry.lookup_key()))
            {
                return Err(frozen());
            }
        }
    }
    for id in changed(&previous.directory_selections, &next.directory_selections) {
        if directories.contains(id.as_str())
            && previous.directory_selections.get(&id) != next.directory_selections.get(&id)
        {
            return Err(frozen());
        }
    }
    for id in changed(
        &previous.directory_tree_selections,
        &next.directory_tree_selections,
    ) {
        let before = selected(previous, &id);
        let after = selected(next, &id);
        let whole = directories.contains(id.as_str());
        let old = protected(
            before.as_ref().map_or(&[], |scope| scope.entries),
            &roots,
            whole,
        );
        let new = protected(
            after.as_ref().map_or(&[], |scope| scope.entries),
            &roots,
            whole,
        );
        if old != new
            || (whole
                && before.as_ref().map(|scope| scope.path)
                    != after.as_ref().map(|scope| scope.path))
        {
            return Err(frozen());
        }
        for entry in old
            .into_iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::File)
        {
            if before
                .as_ref()
                .and_then(|scope| scope.contexts.get(entry.lookup_key()))
                != after
                    .as_ref()
                    .and_then(|scope| scope.contexts.get(entry.lookup_key()))
            {
                return Err(frozen());
            }
        }
    }
    for id in changed(
        &previous.directory_git_repositories,
        &next.directory_git_repositories,
    ) {
        if directories.contains(id.as_str())
            && previous.directory_git_repositories.get(&id)
                != next.directory_git_repositories.get(&id)
        {
            return Err(frozen());
        }
    }
    Ok(())
}
