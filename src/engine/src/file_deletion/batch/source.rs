//! Bind each selected file to its exact current directory composition before
//! collecting whole-file frontiers. No historical branch is silently activated.
use super::*;
use crate::{DirectoryBatchKind, DirectoryBranch, NamespaceEntryKind};
use std::collections::{BTreeMap, BTreeSet};

fn conflict(message: &str) -> Error {
    Error::new(ErrorCode::DirectoryBranchConflict, message)
}

fn require_current(state: &State, branch: &DirectoryBranch) -> Result<()> {
    let git = state
        .directory_git_repositories
        .get(&branch.directory_id)
        .ok_or_else(|| conflict("initialize the selected directory repository first"))?;
    if git.current_branch_id != branch.directory_branch_id
        || state.directory_selections.get(&branch.directory_id) != Some(&branch.directory_branch_id)
    {
        return Err(conflict(
            "deletion requires the selected directory working branch",
        ));
    }
    Ok(())
}

pub(in crate::file_deletion) fn capture(
    state: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
    repository: &str,
    request: &DirectoryNamespaceBatch,
    maximum: usize,
) -> Result<Captured> {
    if request.kind != DirectoryBatchKind::Delete
        || request.items.is_empty()
        || request.items.len() > maximum.min(MAX_FILES)
    {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "file deletion batch requires one to one hundred explicit file occurrences",
        ));
    }
    let repo = state.repositories.get(repository).ok_or_else(|| {
        Error::new(ErrorCode::RepositoryNotFound, "batch repository is missing")
    })?;
    if !repo.file_manager_root {
        return Err(Error::new(
            ErrorCode::FileBranchScope,
            "file deletion batch requires a disk-root history",
        ));
    }
    let main = &state.branches[&repo.main_branch_id];
    let retained = &state.commits[&main.head_commit_id].entries;
    let entries = match current {
        Some(view) => {
            view.require_branch_write(state, &main.branch_id)?;
            view.branch_entries(&main.branch_id, &main.head_commit_id)?
        }
        None => retained,
    };
    let mut seen = BTreeSet::new();
    let mut targets = BTreeMap::new();
    let mut sources = Vec::with_capacity(request.items.len());
    for item in &request.items {
        if item.target.is_some() {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "deletion has no destination",
            ));
        }
        let source = &item.source;
        let path = name_policy::PortablePath::parse(&source.path)?;
        let branch = state
            .directory_branches
            .get(&source.directory_branch_id)
            .filter(|branch| branch.repository_id == repository)
            .ok_or_else(|| conflict("deletion branch is outside this repository"))?;
        if branch.head_tree_id != source.expected_tree_id {
            return Err(conflict(
                "deletion subtree baseline changed; no file was deleted",
            ));
        }
        require_current(state, branch)?;
        let admitted = current
            .map(|view| view.batch_scope(state, &branch.directory_branch_id))
            .transpose()?;
        if admitted.is_none()
            && state
                .directory_tree_selections
                .get(&branch.directory_id)
                .is_some_and(|tree| tree != &branch.head_tree_id)
        {
            return Err(conflict(
                "deletion requires the exact current directory composition",
            ));
        }
        let scope = admitted.as_ref().map_or(branch, |scope| scope.branch());
        crate::business::directory_git::require_owned_path(state, scope, path.as_str())?;
        if !scope.path.is_empty()
            && !path
                .key()
                .starts_with(&format!("{}/", name_policy::path_key(&scope.path)))
        {
            return Err(Error::new(
                ErrorCode::DirectoryBranchScope,
                "file is outside its directory",
            ));
        }
        let entry = scope
            .entries
            .iter()
            .find(|entry| entry.lookup_key() == path.key() && entry.entry_id == source.entry_id)
            .ok_or_else(|| {
                conflict("file identity differs from the selected directory composition")
            })?;
        if entry.kind != NamespaceEntryKind::File {
            return Err(Error::new(
                ErrorCode::IsDirectory,
                "select the files explicitly; directory containers require their own reversible lifecycle",
            ));
        }
        let actual = entries
            .binary_search_by(|entry| entry.lookup_key().cmp(path.key()))
            .ok()
            .map(|index| &entries[index]);
        if actual != Some(entry) {
            return Err(conflict("file is not the exact current namespace occurrence"));
        }
        if state.file_deletions.contains_key(&entry.file_id) {
            return Err(super::super::lifecycle::frozen());
        }
        if !seen.insert((&source.directory_branch_id, &source.entry_id, path.key().to_owned())) {
            return Err(Error::new(
                ErrorCode::BatchMappingConflict,
                "duplicate deletion occurrence",
            ));
        }
        sources.push(FrozenSource {
            tree_id: admitted
                .as_ref()
                .map_or(branch.head_tree_id.as_str(), |scope| scope.selected_tree_id())
                .to_owned(),
            file_id: entry.file_id.clone(),
        });
        // Selecting two visible aliases still deletes one stable file, not two Ds.
        targets
            .entry(entry.file_id.clone())
            .or_insert_with(|| entry.path.clone());
    }
    Ok(Captured { targets, sources })
}
