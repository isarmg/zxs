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
) -> Result<Vec<DeletedPath>> {
    let mut removed = Vec::new();
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
                removed.extend(children.into_iter().map(deleted_path));
                *entries = retained;
            }
        }
    }
    removed.extend(apply_workspace_namespace(entries, current, operations)?);
    Ok(removed)
}
