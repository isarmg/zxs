//! One publication guard covers explicit and implicit file-node producers.
use super::lifecycle::frozen;
use crate::{Entry, NamespaceEntryKind, Result, State};
use storage_contract::MapChanges;

fn changed_keys<T>(
    before: &storage_contract::TrackedMap<T>,
    after: &storage_contract::TrackedMap<T>,
) -> Vec<String> {
    match after.changes_since(before) {
        MapChanges::Keys(keys) => keys.iter().cloned().collect(),
        MapChanges::All => before
            .keys()
            .chain(after.keys())
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
    }
}

fn frozen_entries<'a>(state: &State, entries: &'a [Entry]) -> Vec<&'a Entry> {
    entries
        .iter()
        .filter(|entry| {
            entry.kind == NamespaceEntryKind::File
                && state.file_deletions.contains_key(&entry.file_id)
        })
        .collect()
}

// A removed or retargeted context must not escape via its new branch owner.
// New keys for other files/repositories have no previous occurrence and remain writable.
fn context_path_is_frozen(previous: &State, context_key: &str) -> bool {
    let Some((repository_id, path)) = context_key.split_once('/') else {
        return false;
    };
    let Some(repository) = previous.repositories.get(repository_id) else {
        return false;
    };
    let branch = &previous.branches[&repository.main_branch_id];
    let entries = &previous.commits[&branch.head_commit_id].entries;
    entries
        .binary_search_by(|entry| entry.lookup_key().cmp(path))
        .is_ok_and(|index| {
            let entry = &entries[index];
            entry.kind == NamespaceEntryKind::File
                && previous.file_deletions.contains_key(&entry.file_id)
        })
}

fn verify_selections(previous: &State, next: &State) -> Result<()> {
    // A selection can change without changing the Entry's content version.
    // Freeze its write target and path-local context as well as its body/HEAD.
    for key in changed_keys(&previous.file_selections, &next.file_selections) {
        if previous.file_deletions.contains_key(&key)
            && previous.file_selections.get(&key) != next.file_selections.get(&key)
        {
            return Err(frozen());
        }
    }
    for key in changed_keys(&previous.file_write_origins, &next.file_write_origins) {
        if previous.file_deletions.contains_key(&key)
            && previous.file_write_origins.get(&key) != next.file_write_origins.get(&key)
        {
            return Err(frozen());
        }
    }
    for key in changed_keys(&previous.file_contexts, &next.file_contexts) {
        let before = previous.file_contexts.get(&key);
        let after = next.file_contexts.get(&key);
        if before != after
            && (context_path_is_frozen(previous, &key)
                || before.into_iter().chain(after).any(|context| {
                    [&previous.branches, &next.branches]
                        .into_iter()
                        .any(|branches| {
                            branches.get(&context.branch_id).is_some_and(|branch| {
                                branch
                                    .file_id
                                    .as_ref()
                                    .is_some_and(|file| previous.file_deletions.contains_key(file))
                            })
                        })
                }))
        {
            return Err(frozen());
        }
    }
    Ok(())
}

pub(crate) fn verify(previous: &State, next: &State) -> Result<()> {
    if previous.file_deletions.is_empty() {
        return Ok(());
    }
    // Tracking limits ordinary work to actual changed records. The complete
    // fallback is for decoded recovery data, never a cached trust shortcut.
    macro_rules! direct {
        ($field:ident) => {
            for key in changed_keys(&previous.$field, &next.$field) {
                if previous.$field.get(&key) != next.$field.get(&key)
                    && previous
                        .$field
                        .get(&key)
                        .into_iter()
                        .chain(next.$field.get(&key))
                        .any(|node| previous.file_deletions.contains_key(&node.file_id))
                {
                    return Err(frozen());
                }
            }
        };
    }
    direct!(file_path_nodes);
    direct!(file_metadata_nodes);
    direct!(file_branch_merges);
    verify_selections(previous, next)?;
    // Immutable content uses the persistent records collection rather than a map.
    let keys = match next.versions.changes_since(&previous.versions) {
        MapChanges::Keys(keys) => keys.iter().cloned().collect::<Vec<_>>(),
        MapChanges::All => previous
            .versions
            .keys()
            .chain(next.versions.keys())
            .cloned()
            .collect(),
    };
    for key in keys {
        if previous.versions.get(&key) != next.versions.get(&key)
            && previous
                .versions
                .get(&key)
                .into_iter()
                .chain(next.versions.get(&key))
                .any(|version| previous.file_deletions.contains_key(&version.file_id))
        {
            return Err(frozen());
        }
    }
    for key in changed_keys(&previous.branches, &next.branches) {
        let before = previous.branches.get(&key);
        let after = next.branches.get(&key);
        if before == after {
            continue;
        }
        if before.into_iter().chain(after).any(|branch| {
            branch
                .file_id
                .as_ref()
                .is_some_and(|file| previous.file_deletions.contains_key(file))
        }) {
            return Err(frozen());
        }
        let entries = |state: &State, head: Option<&crate::Branch>| -> Vec<Entry> {
            head.and_then(|branch| state.commits.get(&branch.head_commit_id))
                .map(|commit| {
                    frozen_entries(previous, &commit.entries)
                        .into_iter()
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        if entries(previous, before) != entries(next, after) {
            return Err(frozen());
        }
    }
    for key in changed_keys(&previous.directory_branches, &next.directory_branches) {
        let before = previous.directory_branches.get(&key);
        let after = next.directory_branches.get(&key);
        if frozen_entries(previous, before.map_or(&[], |branch| &branch.entries))
            != frozen_entries(previous, after.map_or(&[], |branch| &branch.entries))
        {
            return Err(frozen());
        }
    }
    Ok(())
}
