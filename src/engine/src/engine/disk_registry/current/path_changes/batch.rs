//! Batch facts consume the explicitly named directory compositions, not an
//! arbitrary historical occurrence or an unrelated alias in the main namespace.
use crate::{DirectoryBranch, Result, State};
use crate::engine::CurrentNamespaces;
use crate::engine::disk_registry::current::path_changes::{Occurrence, invalid};
use crate::persist::path_changes::BatchTransfers;
use std::collections::BTreeMap;

fn scope(
    state: &State,
    current: Option<&CurrentNamespaces>,
    branch: &str,
) -> Result<DirectoryBranch> {
    match current {
        Some(current) => Ok(current.batch_scope(state, branch)?.branch().clone()),
        None => {
            let mut source = state.directory_branches.get(branch).cloned().ok_or_else(invalid)?;
            crate::business::namespace::container_name::normalize_directory_branch(state, &mut source)?;
            Ok(source)
        }
    }
}

pub(super) fn occurrences(
    previous: &State,
    next: &State,
    before: Option<&CurrentNamespaces>,
    after: Option<&CurrentNamespaces>,
    transfers: &BatchTransfers,
) -> Result<(Vec<Occurrence>, Vec<Occurrence>)> {
    let mut old = Vec::new();
    let mut new = Vec::new();
    let mut renamed_files = BTreeMap::new();
    for transfer in &transfers.items {
        let original = previous.directory_branches
            .get(&transfer.source.directory_branch_id).ok_or_else(invalid)?;
        if original.head_tree_id != transfer.source.expected_tree_id
            || original.repository_id != transfers.repository.to_string()
        {
            return Err(invalid());
        }
        let source = scope(previous, before, &transfer.source.directory_branch_id)?;
        let source_path = name_policy::PortablePath::parse(&transfer.source.path)?;
        let selected = source.entries.iter()
            .find(|entry| entry.lookup_key() == source_path.key()).ok_or_else(invalid)?;
        if selected.entry_id != transfer.source.entry_id {
            return Err(invalid());
        }
        match selected.kind {
            crate::NamespaceEntryKind::File => {
                if transfer.file_id != Some(selected.file_id.parse()?)
                    || transfer.version_id != Some(selected.version_id.parse()?)
                {
                    return Err(invalid());
                }
            }
            crate::NamespaceEntryKind::Directory => {
                if transfer.file_id.is_some() || transfer.version_id.is_some() {
                    return Err(invalid());
                }
            }
        }
        let target = transfer.target.as_ref().ok_or_else(invalid)?;
        let initial_target = previous.directory_branches
            .get(&target.directory_branch_id).ok_or_else(invalid)?;
        if initial_target.head_tree_id != target.expected_tree_id
            || initial_target.repository_id != source.repository_id
        {
            return Err(invalid());
        }
        let destination = scope(next, after, &target.directory_branch_id)?;
        if destination.repository_id != source.repository_id {
            return Err(invalid());
        }
        let destinations: BTreeMap<_, _> = destination.entries.iter()
            .map(|entry| (entry.path(), entry)).collect();
        let target_path = name_policy::PortablePath::parse(&target.path)?;
        if selected.kind == crate::NamespaceEntryKind::File
            && matches!(transfers.kind, crate::DirectoryBatchKind::Rename | crate::DirectoryBatchKind::Move)
        {
            let file = selected.file_id.parse::<storage_contract::FileId>()?;
            let name = name_policy::Name::parse(target_path.file_name())?;
            if crate::container_storage::current_container_name(next, &selected.file_id)? != name {
                return Err(invalid());
            }
            if let Some(previous) = renamed_files.insert(file, name.clone())
                && previous != name
            {
                return Err(invalid());
            }
        }
        let repository = source.repository_id.parse()?;
        let prefix = format!("{}/", selected.path());
        for entry in &source.entries {
            if entry.kind != crate::NamespaceEntryKind::File {
                continue;
            }
            let suffix = if entry.path() == selected.path() {
                ""
            } else if let Some(suffix) = entry.path().strip_prefix(&prefix) {
                suffix
            } else {
                continue;
            };
            let path = if suffix.is_empty() {
                target_path.as_str().to_owned()
            } else {
                format!("{}/{suffix}", target_path.as_str())
            };
            if matches!(
                transfers.kind,
                crate::DirectoryBatchKind::Copy | crate::DirectoryBatchKind::Separate
            ) {
                // These requests allocate a new container and carry their own
                // retained source proof, not a same-container path fact.
                continue;
            }
            let moved = destinations.get(path.as_str()).copied().filter(|candidate| {
                candidate.entry_id == entry.entry_id
                    && candidate.file_id == entry.file_id
            }).ok_or_else(invalid)?;
            old.push(Occurrence {
                repository,
                entry: entry.clone(),
                branch_id: source.file_contexts.get(entry.lookup_key())
                    .map(|context| context.branch_id.parse()).transpose()?,
            });
            new.push(Occurrence {
                repository,
                entry: moved.clone(),
                branch_id: destination.file_contexts.get(moved.lookup_key())
                    .map(|context| context.branch_id.parse()).transpose()?,
            });
        }
    }
    // A rename is container-wide. Bind every additional live occurrence to
    // the same request-selected file and preserve its parent, body and identity.
    let before_live = super::occurrences(previous, before)?;
    let after_live = super::occurrences(next, after)?;
    for occurrence in before_live {
        if occurrence.repository != transfers.repository
            || old.iter().any(|source| source.repository == occurrence.repository
                && source.entry == occurrence.entry
                && source.branch_id == occurrence.branch_id)
        {
            continue;
        }
        let file = occurrence.entry.file_id.parse::<storage_contract::FileId>()?;
        let Some(name) = renamed_files.get(&file) else { continue; };
        let mut expected = occurrence.entry.clone();
        let path = match expected.location().parent() {
            Some(parent) => parent.join_name(name)?,
            None => name_policy::PortablePath::from_name(name)?,
        };
        if path == expected.path {
            continue;
        }
        expected.relocate(path);
        let target = after_live.iter().find(|target| {
            target.repository == occurrence.repository && target.entry == expected
        }).ok_or_else(invalid)?;
        new.push(Occurrence {
            repository: target.repository,
            entry: target.entry.clone(),
            branch_id: target.branch_id,
        });
        old.push(occurrence);
    }
    Ok((old, new))
}
