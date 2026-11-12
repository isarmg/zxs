//! Batch facts consume the explicitly named directory compositions, not an
//! arbitrary historical occurrence or an unrelated alias in the main namespace.
use super::{CurrentNamespaces, Occurrence, Result, State, invalid};
use crate::DirectoryBranch;
use crate::persist::path_changes::BatchTransfers;

fn scope(
    state: &State,
    current: Option<&CurrentNamespaces>,
    branch: &str,
) -> Result<DirectoryBranch> {
    match current {
        Some(current) => Ok(current.batch_scope(state, branch)?.branch().clone()),
        None => state.directory_branches.get(branch).cloned().ok_or_else(invalid),
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
    for transfer in &transfers.items {
        let original = previous.directory_branches
            .get(&transfer.source.directory_branch_id).ok_or_else(invalid)?;
        if original.head_tree_id != transfer.source.expected_tree_id
            || original.repository_id != transfers.repository.to_string()
        {
            return Err(invalid());
        }
        let source = scope(previous, before, &transfer.source.directory_branch_id)?;
        let selected = source.entries.iter()
            .find(|entry| entry.path() == transfer.source.path).ok_or_else(invalid)?;
        if selected.entry_id != transfer.source.entry_id
            || selected.file_id != transfer.file_id
            || selected.version_id != transfer.version_id
        {
            return Err(invalid());
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
                target.path.clone()
            } else {
                format!("{}/{suffix}", target.path)
            };
            if matches!(
                transfers.kind,
                crate::DirectoryBatchKind::Copy | crate::DirectoryBatchKind::Separate
            ) {
                // These requests allocate a new container and carry their own
                // retained source proof, not a same-container path fact.
                continue;
            }
            let moved = destination.entries.iter().find(|candidate| {
                candidate.path() == path
                    && candidate.entry_id == entry.entry_id
                    && candidate.file_id == entry.file_id
            }).ok_or_else(invalid)?;
            old.push(Occurrence {
                repository,
                entry: entry.clone(),
                branch_id: source.file_contexts.get(entry.lookup_key())
                    .map(|context| context.branch_id.clone()),
            });
            new.push(Occurrence {
                repository,
                entry: moved.clone(),
                branch_id: destination.file_contexts.get(moved.lookup_key())
                    .map(|context| context.branch_id.clone()),
            });
        }
    }
    Ok((old, new))
}
