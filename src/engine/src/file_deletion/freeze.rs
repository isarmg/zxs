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
    let mut paths = std::collections::BTreeSet::new();
    for entry in entries.iter().filter(|entry| {
        entry.kind == NamespaceEntryKind::File && state.file_deletions.contains_key(&entry.file_id)
    }) {
        let mut path = entry.lookup_key();
        paths.insert(path);
        while let Some((parent, _)) = path.rsplit_once('/') {
            paths.insert(parent);
            path = parent;
        }
    }
    // A replacement ancestor must not silently change the deleted occurrence's
    // stable directory ownership while leaving its file Entry byte-identical.
    entries
        .iter()
        .filter(|entry| paths.contains(entry.lookup_key()))
        .collect()
}

fn verify_directory_contexts(
    previous: &State,
    before: Option<&crate::DirectoryBranch>,
    after: Option<&crate::DirectoryBranch>,
) -> Result<()> {
    let (Some(before), Some(after)) = (before, after) else {
        return Ok(()); // Frozen occurrence additions/removals are checked first.
    };
    for entry in before.entries.iter().filter(|entry| {
        entry.kind == NamespaceEntryKind::File
            && previous.file_deletions.contains_key(&entry.file_id)
    }) {
        if before.repository_id != after.repository_id
            || before.directory_id != after.directory_id
            || before.file_contexts.get(entry.lookup_key())
                != after.file_contexts.get(entry.lookup_key())
            || before.file_branches.get(&entry.file_id) != after.file_branches.get(&entry.file_id)
            || before.inherited_files.get(&entry.file_id)
                != after.inherited_files.get(&entry.file_id)
        {
            return Err(frozen());
        }
    }
    Ok(())
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

fn verify_selections(
    previous: &State,
    next: &State,
    removals: &[(&crate::Branch, crate::business::namespace::current_removal::CurrentRemoval)],
) -> Result<()> {
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
        let removed_mount = before.is_some() && after.is_none()
            && removals.iter().any(|(_, proof)| proof.removes_context(&key));
        if before != after && !removed_mount
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

fn verify_notes(previous: &State, next: &State) -> Result<()> {
    let keys = match next.container_tags.changes_since(&previous.container_tags) {
        MapChanges::Keys(keys) => (*keys).clone(),
        MapChanges::All => previous.container_tags.keys()
            .chain(next.container_tags.keys()).copied().collect(),
    };
    for key in keys {
        if previous.container_tags.get(&key) != next.container_tags.get(&key)
            && previous.file_deletions.contains_key(&key.to_string())
        {
            return Err(frozen());
        }
    }

    for key in changed_keys(&previous.file_notes, &next.file_notes) {
        if previous.file_notes.get(&key) != next.file_notes.get(&key)
            && previous.file_deletions.contains_key(&key)
        {
            return Err(frozen());
        }
    }
    for key in changed_keys(&previous.version_notes, &next.version_notes) {
        if previous.version_notes.get(&key) != next.version_notes.get(&key)
            && previous
                .versions
                .get(&key)
                .is_some_and(|version| previous.file_deletions.contains_key(&version.file_id))
        {
            return Err(frozen());
        }
    }
    // These note owners are immutable retained nodes. The direct node guards
    // reject creating, removing or retargeting a frozen owner's node first.
    for key in changed_keys(&previous.file_path_notes, &next.file_path_notes) {
        let owner = previous
            .file_path_nodes
            .get(&key)
            .map(|node| &node.file_id)
            .or_else(|| {
                previous
                    .file_metadata_nodes
                    .get(&key)
                    .map(|node| &node.file_id)
            });
        if previous.file_path_notes.get(&key) != next.file_path_notes.get(&key)
            && owner.is_some_and(|file| previous.file_deletions.contains_key(file))
        {
            return Err(frozen());
        }
    }
    Ok(())
}

pub(crate) fn verify(previous: &State, next: &State) -> Result<()> {
    verify_with_removal(previous, next, None, false)
}

pub(crate) fn verify_current(
    previous: &State,
    next: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
) -> Result<()> {
    verify_with_removal(previous, next, current, true)
}

fn verify_with_removal(
    previous: &State,
    next: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
    allow_current_removal: bool,
) -> Result<()> {
    super::directory::verify_frozen(previous, next)?;
    if previous.file_deletions.is_empty() {
        return Ok(());
    }
    let mut removals = Vec::new();
    if allow_current_removal {
        for key in changed_keys(&previous.branches, &next.branches) {
            if let (Some(before), Some(after)) = (
                previous.branches.get(&key), next.branches.get(&key),
            ) && before.file_id.is_none() && after.file_id.is_none()
                && previous.commits.get(&before.head_commit_id).is_some_and(|source| {
                    next.commits.get(&after.head_commit_id).is_some_and(|target| {
                        frozen_entries(previous, &source.entries)
                            != frozen_entries(previous, &target.entries)
                    })
                })
                && let Some(proof) =
                crate::business::namespace::current_removal::verified_removed_rows(
                    previous, next, current, before, after,
                )?
            {
                removals.push((before, proof));
            }
        }
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
    verify_notes(previous, next)?;
    verify_selections(previous, next, &removals)?;
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
        let old_entries = entries(previous, before);
        let new_entries = entries(next, after);
        if old_entries != new_entries {
            if !removals.iter().any(|(branch, _)| branch.branch_id == key) {
                return Err(frozen());
            }
            // The proof compares the entire retained namespace, including every
            // ancestor that stops appearing in frozen_entries after this removal.
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
        verify_directory_contexts(previous, before, after)?;
    }
    Ok(())
}
