//! Ordinary namespace path and selection projection. The caller supplies the
//! admitted before/after occurrences of one native owner; a directory name
//! carries no independent history, index, or authority to retarget a FILE.
use crate::business::{file_paths, namespace};
use namespace::file_context::{context_at_key, context_key_from_key};
use crate::{Entry, Error, NamespaceEntryKind, NamespaceOperation, NamespaceOperationKind,
    OperationId, Result, State};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    Rename,
    Move,
}

/// Rebase the physical container using this exact occurrence and selected
/// source, without changing any immutable version's original body or name.
pub(crate) fn physical_path_after(
    old: String,
    file: &Entry,
    source: &str,
    destination: &str,
    operation: Change,
    is_directory: bool,
) -> String {
    let same_parent = name_policy::path_key(source.rsplit_once('/').map(|v| v.0).unwrap_or(""))
        == name_policy::path_key(destination.rsplit_once('/').map(|v| v.0).unwrap_or(""));
    if operation == Change::Rename && same_parent {
        if is_directory {
            let key = name_policy::path_key(source);
            let owner = name_policy::path_key(&old);
            if namespace::is_descendant(&owner, &key) {
                let depth = source.split('/').count();
                format!(
                    "{destination}/{}",
                    old.split('/').skip(depth).collect::<Vec<_>>().join("/")
                )
            } else {
                old
            }
        } else {
            let name = destination.rsplit('/').next().unwrap_or(destination);
            old.rsplit_once('/')
                .map(|(parent, _)| format!("{parent}/{name}"))
                .unwrap_or_else(|| name.into())
        }
    } else {
        let depth = source.split('/').count();
        let suffix = file
            .path()
            .split('/')
            .skip(depth)
            .collect::<Vec<_>>()
            .join("/");
        if suffix.is_empty() {
            destination.into()
        } else {
            format!("{destination}/{suffix}")
        }
    }
}

/// Rebind only surviving stable Entry/FILE pairs; a body version can advance
/// only through the ordinary writer. Non-content changes retain path facts.
pub(in crate::business) fn apply(
    state: &mut State,
    repository: &str,
    before: &[Entry],
    after: &[Entry],
    changes: &[NamespaceOperation],
    principal: &str,
    operation: OperationId,
) -> Result<Vec<crate::persist::path_changes::PathChanged>> {
    if changes.iter().any(|change| {
        matches!(
            change,
            NamespaceOperation::Delete { .. } | NamespaceOperation::DeleteTree { .. }
        )
    }) {
        crate::file_deletion::preflight::require_reversible_removals(
            state, repository, before, after,
        )?;
    }
    // Workspace entries may not be sorted yet. Keep the first path match,
    // just like `find`, without rescanning all siblings for every file.
    let mut after_by_path = BTreeMap::new();
    for entry in after {
        after_by_path.entry(entry.lookup_key()).or_insert(entry);
    }
    let original_contexts = before
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::File)
        .filter_map(|e| {
            context_at_key(state, repository, e.lookup_key())
                .map(|c| (context_key_from_key(repository, e.lookup_key()), c))
        })
        .collect::<BTreeMap<_, _>>();
    let mut physical_outcomes = BTreeMap::<String, String>::new();
    let mut contexts = BTreeMap::new();
    let mut path_changes = Vec::new();
    let operation_files = namespace::container_name::operation_files(before, changes)?;
    for old in before.iter().filter(|e| e.kind == NamespaceEntryKind::File) {
        // Replay each entry's path independently. Looking only at the first
        // rename loses chained moves; matching by file ID alone confuses aliases.
        let mut moved = old.clone();
        let mut renamed = false;
        let mut deleted = false;
        let mut physical_path = state
            .file_locations
            .get(&old.file_id)
            .cloned()
            .unwrap_or_else(|| old.path().to_owned());
        let original_physical_path = physical_path.clone();
        for (change, renamed_file) in changes.iter().zip(&operation_files) {
            let source = change.key();
            if (change.kind() == NamespaceOperationKind::Delete && moved.lookup_key() == source)
                || (change.kind() == NamespaceOperationKind::DeleteTree
                    && (moved.lookup_key() == source
                        || namespace::is_descendant(moved.lookup_key(), source)))
            {
                deleted = true;
                break;
            }
            let NamespaceOperation::Rename { source: from, destination } = change else {
                continue;
            };
            let direct = moved.lookup_key() == source
                || namespace::is_descendant(moved.lookup_key(), source);
            let Some(path) = namespace::container_name::renamed_path(
                &moved, from, destination, *renamed_file,
            )? else {
                continue;
            };
            if path.as_str() == moved.path() {
                continue;
            }
            if renamed_file.is_some_and(|file| old.file_id == file.to_string()) {
                // Every occurrence follows the same physical outcome, even if
                // a later link in this operation is addressed through another alias.
                physical_path = namespace::container_name::renamed_file_location(
                    &name_policy::PortablePath::parse(&physical_path)?, from, destination,
                )?.as_str().to_owned();
                renamed = true;
            } else if direct {
                physical_path = physical_path_after(
                    physical_path,
                    &moved,
                    change.path(),
                    destination.as_str(),
                    Change::Rename,
                    moved.lookup_key() != source,
                );
                renamed = true;
            }
            moved.relocate(path);
        }
        if !deleted
            && let Some(current) = after_by_path.get(moved.lookup_key()).filter(|e| {
                e.kind == NamespaceEntryKind::File
                    && e.entry_id == old.entry_id
                    && e.file_id == old.file_id
            })
        {
            let context = original_contexts
                .get(&context_key_from_key(repository, old.lookup_key()))
                .cloned();
            let context = if current.path() != old.path() {
                let changed = file_paths::record_change(
                    state,
                    repository,
                    file_paths::PathChange {
                        old,
                        new: current,
                        after_entries: after,
                        context,
                    },
                    principal,
                    operation,
                )?;
                path_changes.push(changed.fact);
                Some(changed.context)
            } else {
                state
                    .file_contexts
                    .get(&context_key_from_key(repository, old.lookup_key()))
                    .cloned()
            };
            if let Some(context) = context {
                contexts.insert(
                    context_key_from_key(repository, current.lookup_key()),
                    context,
                );
            }
            // Renaming a non-owning alias directory does not propose a
            // competing physical location. Compare net outcomes so a
            // temporary move followed by its reversal is also a no-op.
            if renamed && physical_path != original_physical_path {
                let outcome = physical_path;
                if let Some(previous) =
                    physical_outcomes.insert(old.file_id.clone(), outcome.clone())
                    && name_policy::path_key(&previous) != name_policy::path_key(&outcome)
                {
                    return Err(Error::new(
                        storage_contract::ErrorCode::BatchMappingConflict,
                        "namespace changes give one physical file incompatible names or locations",
                    ));
                }
            }
        }
    }
    for (file, path) in physical_outcomes {
        state
            .file_names
            .insert(file.clone(), path.rsplit('/').next().unwrap_or("").into());
        state.file_locations.insert(file, path);
    }
    // Remove old keys together before installing new keys, so a path swap
    // cannot erase the context already installed for the other file.
    for old in before.iter().filter(|e| e.kind == NamespaceEntryKind::File) {
        state
            .file_contexts
            .remove(&context_key_from_key(repository, old.lookup_key()));
    }
    state.file_contexts.extend(contexts);
    let live = after
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::File)
        .map(|e| e.file_id.as_str())
        .collect::<BTreeSet<_>>();
    for old in before
        .iter()
        .filter(|e| e.kind == NamespaceEntryKind::File && !live.contains(e.file_id.as_str()))
    {
        state.file_selections.remove(&old.file_id);
    }
    Ok(path_changes)
}
