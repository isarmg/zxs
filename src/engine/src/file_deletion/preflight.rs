use super::*;
use crate::{BranchKind, NamespaceEntryKind, business::file_nodes};
use std::collections::BTreeSet;

pub(super) fn checkpoint(cancellation: &HistoryWindowCancellation) -> Result<()> {
    if cancellation.is_cancelled() {
        return Err(Error::new(
            ErrorCode::OperationAborted,
            "file deletion preflight cancelled",
        ));
    }
    Ok(())
}

fn limit() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "complete file deletion preflight exceeds the current entry budget",
    )
}

fn scope() -> Error {
    Error::new(
        ErrorCode::FileBranchScope,
        "file deletion preflight requires a disk-root file outside initialized directory repositories",
    )
}

fn entry_at<'a>(entries: &'a [Entry], key: &str) -> Option<&'a Entry> {
    entries
        .binary_search_by(|entry| entry.lookup_key().cmp(key))
        .ok()
        .map(|index| &entries[index])
}

fn require_ordinary_occurrence(
    state: &State,
    entries: &[Entry],
    entry: &Entry,
    cancellation: &HistoryWindowCancellation,
) -> Result<()> {
    let mut key = entry.lookup_key();
    while let Some((parent, _)) = key.rsplit_once('/') {
        checkpoint(cancellation)?;
        let directory = entry_at(entries, parent)
            .expect("invariant: admitted namespace entries have directory parents");
        if state
            .directory_git_repositories
            .contains_key(&directory.entry_id)
        {
            return Err(scope());
        }
        key = parent;
    }
    Ok(())
}

fn occurrences<'a>(
    state: &State,
    entries: &'a [Entry],
    file: &str,
    maximum: usize,
    cancellation: &HistoryWindowCancellation,
) -> Result<Vec<&'a Entry>> {
    let mut occurrences = Vec::new();
    for entry in entries {
        checkpoint(cancellation)?;
        if entry.kind != NamespaceEntryKind::File || entry.file_id != file {
            continue;
        }
        if occurrences.len() == maximum {
            return Err(limit());
        }
        require_ordinary_occurrence(state, entries, entry, cancellation)?;
        occurrences.push(entry);
    }
    Ok(occurrences)
}

fn branch_heads<'a>(
    state: &'a State,
    repository: &Repository,
    file: &str,
    maximum: usize,
    cancellation: &HistoryWindowCancellation,
) -> Result<Vec<FileDeletionBranchHead<'a>>> {
    let mut branches = Vec::new();
    for branch in state.branches.values() {
        checkpoint(cancellation)?;
        if branch.file_id.as_deref() != Some(file) {
            continue;
        }
        if branch.history_tree_id != repository.history_tree_id
            || branch.branch_kind != BranchKind::File
        {
            return Err(Error::new(
                ErrorCode::FileBranchScope,
                "retained file branches do not form one independently owned history scope",
            ));
        }
        if branches.len() == maximum {
            return Err(limit());
        }
        let node_id = file_nodes::branch_head(state, &branch.branch_id)?;
        let head = FileDeletionHead::retained(state, &node_id);
        if head.history_tree_id() != repository.history_tree_id
            || head.body().history_tree_id != repository.history_tree_id
        {
            return Err(Error::new(
                ErrorCode::FileBranchScope,
                "retained file branch head is outside its history scope",
            ));
        }
        let alias_of = state
            .file_branch_aliases
            .get(&branch.branch_id)
            .map(String::as_str);
        if let Some(alias) = alias_of {
            let survivor = state.branches.get(alias).ok_or_else(|| {
                Error::new(
                    ErrorCode::StateInvalid,
                    "retained file branch alias is missing",
                )
            })?;
            if survivor.file_id.as_deref() != Some(file)
                || survivor.history_tree_id != repository.history_tree_id
                || survivor.branch_kind != BranchKind::File
            {
                return Err(Error::new(
                    ErrorCode::FileBranchScope,
                    "retained file branch alias crosses file ownership",
                ));
            }
        }
        branches.push(FileDeletionBranchHead {
            branch,
            head,
            alias_of,
        });
    }
    if branches.is_empty() {
        return Err(Error::new(
            ErrorCode::FileBranchRequired,
            "file deletion preflight requires independently recorded file branches; it does not initialize them",
        ));
    }
    Ok(branches)
}

pub(crate) fn capture<'a>(
    state: &'a State,
    repository: &'a Repository,
    entries: &'a [Entry],
    target: FileDeletionTarget<'_>,
    maximum: usize,
    cancellation: &HistoryWindowCancellation,
) -> Result<FileDeletionPreflight<'a>> {
    checkpoint(cancellation)?;
    let selected = entry_at(entries, target.path.key())
        .filter(|entry| {
            entry.kind == NamespaceEntryKind::File && entry.file_id == target.file_id.as_str()
        })
        .ok_or_else(|| {
            Error::new(
                ErrorCode::FileNotFound,
                "file is not present at this logical path",
            )
        })?;
    if !repository.file_manager_root
        || state
            .directory_git_repositories
            .contains_key(&repository.repository_id)
    {
        return Err(scope());
    }
    let occurrences = occurrences(state, entries, &selected.file_id, maximum, cancellation)?;
    let branches = branch_heads(state, repository, &selected.file_id, maximum, cancellation)?;
    let mut parents = BTreeSet::new();
    for branch in &branches {
        checkpoint(cancellation)?;
        parents.insert(branch.node_id().to_owned());
    }
    Ok(FileDeletionPreflight {
        repository,
        file_id: &selected.file_id,
        sequence: state.sequence,
        branches,
        occurrences,
        parent_node_ids: parents.into_iter().collect(),
    })
}
