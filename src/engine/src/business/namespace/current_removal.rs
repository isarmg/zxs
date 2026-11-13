//! Remove only already soft-deleted mounts beneath one empty ordinary directory.
//! Historical rows and FILE deletion facts remain owned by their original records.
use crate::business::namespace::{
    apply_workspace_namespace, deleted_path, find_unordered_key, is_descendant,
};
use crate::Result;
use storage_contract::{
    Branch, DeletedPath, Entry, NamespaceEntryKind, NamespaceOperation, Repository, State,
};

pub(crate) fn apply_current_workspace_namespace(
    state: &State,
    repository: &Repository,
    branch: &Branch,
    entries: &mut Vec<Entry>,
    current: &[Entry],
    operations: &[NamespaceOperation],
) -> Result<(Vec<DeletedPath>, Option<CurrentRemoval>)> {
    let mut removed = Vec::new();
    let mut proof = None;
    if let [NamespaceOperation::Delete { path }] = operations {
        let root = find_unordered_key(current, path.key());
        let ordinary = repository.file_manager_root && repository.main_branch_id == branch.branch_id
            && root.is_some_and(|entry| entry.kind == NamespaceEntryKind::Directory)
            && entries.as_slice() == current;
        if ordinary {
            let children = entries.iter()
                .filter(|entry| is_descendant(entry.lookup_key(), path.key()))
                .collect::<Vec<_>>();
            // The admitted raw Current rows identify the exact occurrences.
            // D applies to all occurrences of its stable FILE; a hidden parent,
            // unavailable disk or a historical name is not a deletion proof.
            let all_deleted = children.iter().all(|entry| {
                entry.kind == NamespaceEntryKind::File
                    && state.file_deletions.get(&entry.file_id).is_some_and(|deletion| {
                        deletion.repository_id == repository.repository_id
                            && deletion.file_id == entry.file_id
                    })
            });
            if all_deleted && !children.is_empty() {
                let retained = entries.iter().filter(|entry| {
                    !is_descendant(entry.lookup_key(), path.key())
                }).cloned().collect::<Vec<_>>();
                // This existing guard rejects initialized directory owners.
                // They must keep their reversible directory lifecycle instead.
                crate::file_deletion::preflight::require_reversible_removals(
                    state, &repository.repository_id, current, &retained,
                )?;
                proof = Some(CurrentRemoval {
                    repository: repository.clone(),
                    rows: children.iter().map(|entry| (**entry).clone()).collect(),
                });
                removed.extend(children.into_iter().map(deleted_path));
                *entries = retained;
            }
        }
    }
    removed.extend(apply_workspace_namespace(entries, current, operations)?);
    Ok((removed, proof))
}

/// Only the admitted single ordinary-directory removal can mint this capability.
/// The removed mounts stop being Current rows, not frozen FILE state.
pub(crate) struct CurrentRemoval {
    repository: Repository,
    rows: Vec<Entry>,
}

impl CurrentRemoval {
    pub(crate) fn removes_context(&self, key: &str) -> bool {
        let Some(path) = key.strip_prefix(&self.repository.repository_id)
            .and_then(|suffix| suffix.strip_prefix('/'))
        else {
            return false;
        };
        // Admission keeps Current rows ordered; selecting descendants preserves it.
        self.rows.binary_search_by(|entry| entry.lookup_key().cmp(path)).is_ok()
    }

    pub(crate) fn preserve_file_state(&self, previous: &State, next: &mut State) {
        for entry in &self.rows {
            if let Some(selection) = previous.file_selections.get(&entry.file_id) {
                next.file_selections.insert(entry.file_id.clone(), selection.clone());
            }
            if let Some(origin) = previous.file_write_origins.get(&entry.file_id) {
                next.file_write_origins.insert(entry.file_id.clone(), origin.clone());
            }
        }
    }
}

/// Reconstruct the same narrow capability from the existing namespace commit.
/// This is used at planning and Current replay; no additional ledger is created.
pub(crate) fn verified_removed_rows(
    previous: &State,
    next: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
    before: &Branch,
    after: &Branch,
) -> Result<Option<CurrentRemoval>> {
    if before.file_id.is_some() || after.file_id.is_some() {
        return Ok(None);
    }
    let Some(repository) = previous.repositories.values().find(|repository| {
        repository.file_manager_root && repository.main_branch_id == before.branch_id
    }) else {
        return Ok(None);
    };
    let (Some(source), Some(target)) = (
        previous.commits.get(&before.head_commit_id),
        next.commits.get(&after.head_commit_id),
    ) else {
        return Ok(None);
    };
    if source.commit_id == target.commit_id
        || next.commits.get(&source.commit_id) != Some(source)
        || target.parent_commit_ids != vec![source.commit_id.clone()]
        || target.history_tree_id != source.history_tree_id
        || target.kind != "content"
        || target.sequence != next.sequence
        || target.pending_file_versions != source.pending_file_versions
        || previous.file_deletions != next.file_deletions
    {
        return Ok(None);
    }
    let mut expected_branch = before.clone();
    let Ok(generation) = storage_contract::Generation::new(before.generation).next() else {
        return Ok(None);
    };
    expected_branch.head_commit_id = target.commit_id.clone();
    expected_branch.generation = generation.get();
    if &expected_branch != after {
        return Ok(None);
    }
    let entries = match current {
        Some(view) => view.branch_entries(&before.branch_id, &source.commit_id)?,
        None => source.entries.as_slice(),
    };
    // Current may carry admitted canonical names newer than its immutable
    // source commit. Rebuild the complete target from that exact head-bound
    // Current view; never copy frozen historical paths back into live scope.
    let retained_ids = target.entries.iter().map(|entry| entry.entry_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let removed = entries.iter().filter(|entry| {
        !retained_ids.contains(entry.entry_id.as_str())
    }).cloned().collect::<Vec<_>>();
    let mut directories = removed.iter().filter(|entry| {
        entry.kind == NamespaceEntryKind::Directory
    });
    let Some(root) = directories.next() else {
        return Ok(None);
    };
    if directories.next().is_some() {
        return Ok(None);
    }
    if removed.iter().any(|entry| {
        entry != root && (entry.kind != NamespaceEntryKind::File
            || !is_descendant(entry.lookup_key(), root.lookup_key())
            || !previous.file_deletions.get(&entry.file_id).is_some_and(|deletion| {
                deletion.repository_id == repository.repository_id
                    && deletion.file_id == entry.file_id
            }))
    }) {
        return Ok(None);
    }
    let operation = NamespaceOperation::Delete {
        path: name_policy::LookupPath::parse(root.path())?,
    };
    let mut expected_entries = entries.to_vec();
    let (deletions, proof) = apply_current_workspace_namespace(
        previous, repository, before, &mut expected_entries, entries, &[operation],
    )?;
    if proof.is_none() || expected_entries != target.entries
        || !target.namespace_deletions.as_ref().is_some_and(|record| record.paths == deletions)
    {
        return Ok(None);
    }
    Ok(proof)
}
