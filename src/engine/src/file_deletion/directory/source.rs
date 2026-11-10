//! Capture only a complete, selected ordinary subtree. A nested initialized
//! repository is a separate identity and cannot be silently absorbed by a delete.
use super::*;
use crate::{BranchState, DirectoryBranch};
use std::collections::BTreeSet;

pub(super) struct Captured<'a> {
    pub(super) entry: Entry,
    pub(super) owner_directory_id: String,
    pub(super) namespace_head: String,
    pub(super) namespace: &'a [Entry],
    pub(super) branch: String,
    pub(super) tree: String,
    pub(super) files: BTreeMap<String, name_policy::PortablePath>,
}

pub(super) fn within(path: &str, root: &str) -> bool {
    path == root || path.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
}

pub(super) fn at<'a>(entries: &'a [Entry], key: &str) -> Option<&'a Entry> {
    entries.binary_search_by(|entry| entry.lookup_key().cmp(key))
        .ok().map(|index| &entries[index])
}

pub(super) fn subtree<'a>(entries: &'a [Entry], key: &str) -> Vec<&'a Entry> {
    let mut selected = Vec::new();
    if let Some(root) = at(entries, key) {
        selected.push(root);
    }
    let prefix = format!("{key}/");
    let start = entries.partition_point(|entry| entry.lookup_key() < prefix.as_str());
    selected.extend(entries[start..].iter()
        .take_while(|entry| entry.lookup_key().starts_with(&prefix)));
    selected
}

fn conflict(message: &str) -> Error {
    Error::new(ErrorCode::DirectoryBranchConflict, message)
}

fn selected<'a>(
    state: &'a State,
    current: Option<&crate::engine::CurrentNamespaces>,
    repository: &str,
    branch: &str,
) -> Result<&'a DirectoryBranch> {
    let branch = state.directory_branches.get(branch)
        .filter(|branch| branch.repository_id == repository)
        .ok_or_else(|| conflict("directory deletion owner differs"))?;
    let git = state.directory_git_repositories.get(&branch.directory_id)
        .ok_or_else(|| conflict("initialize the owning directory repository first"))?;
    if git.repository_id != repository
        || git.current_branch_id != branch.directory_branch_id
        || state.directory_selections.get(&branch.directory_id) != Some(&branch.directory_branch_id)
    {
        return Err(conflict("directory deletion requires the selected working owner"));
    }
    if let Some(view) = current {
        view.require_directory_branch_write(state, &branch.directory_branch_id)?;
    } else if state.directory_tree_selections.get(&branch.directory_id)
        .is_some_and(|tree| tree != &branch.head_tree_id)
    {
        return Err(conflict("return to the current working composition before deleting or restoring"));
    }
    Ok(branch)
}

pub(super) fn namespace<'a>(
    state: &'a State,
    current: Option<&'a crate::engine::CurrentNamespaces>,
    repository: &str,
) -> Result<(&'a Repository, &'a crate::Commit, &'a [Entry])> {
    let repo = state.repositories.get(repository)
        .ok_or_else(|| Error::new(ErrorCode::RepositoryNotFound, "repository missing"))?;
    let branch = &state.branches[&repo.main_branch_id];
    if !repo.file_manager_root || branch.state != BranchState::Active {
        return Err(Error::new(
            ErrorCode::FileBranchScope,
            "directory lifecycle requires a writable disk-root history",
        ));
    }
    let head = &state.commits[&branch.head_commit_id];
    let entries = match current {
        Some(view) => {
            view.require_branch_write(state, &branch.branch_id)?;
            view.branch_entries(&branch.branch_id, &head.commit_id)?
        }
        None => &head.entries,
    };
    Ok((repo, head, entries))
}

pub(super) fn capture<'a>(
    state: &'a State,
    current: Option<&'a crate::engine::CurrentNamespaces>,
    input: &DeleteInput,
    maximum: usize,
) -> Result<Captured<'a>> {
    if input.request.kind != DirectoryBatchKind::Delete
        || input.request.items.len() != 1
        || input.request.items[0].target.is_some()
    {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "directory deletion requires one explicit directory and no destination",
        ));
    }
    let source = &input.request.items[0].source;
    let path = name_policy::PortablePath::parse(&source.path)?;
    let branch = selected(state, current, &input.repository_id, &source.directory_branch_id)?;
    if branch.head_tree_id != source.expected_tree_id {
        return Err(conflict("directory working baseline changed; no deletion was published"));
    }
    let admitted = current.map(|view| view.batch_scope(state, &branch.directory_branch_id))
        .transpose()?;
    let scope = admitted.as_ref().map_or(branch, |scope| scope.branch());
    crate::business::directory_git::require_owned_path(state, scope, path.as_str())?;
    if !scope.path.is_empty()
        && !path.key().strip_prefix(&name_policy::path_key(&scope.path))
            .is_some_and(|rest| rest.starts_with('/'))
    {
        return Err(Error::new(ErrorCode::DirectoryBranchScope, "directory is outside its owner"));
    }
    let entry = at(&scope.entries, path.key())
        .filter(|entry| entry.entry_id == source.entry_id
            && entry.kind == NamespaceEntryKind::Directory)
        .ok_or_else(|| conflict("directory identity differs from the selected composition"))?;
    let (_, head, namespace) = namespace(state, current, &input.repository_id)?;
    let original = subtree(&head.entries, path.key());
    let projected = subtree(namespace, path.key());
    if original != projected || projected != subtree(&scope.entries, path.key())
        || at(namespace, path.key()) != Some(entry)
        || original.len() > maximum
    {
        return Err(conflict("directory deletion requires its complete current subtree"));
    }
    let owner = &state.directory_git_repositories[&branch.directory_id];
    if owner.deletions.values().any(|node| {
        within(path.key(), node.path.key()) || within(node.path.key(), path.key())
    }) {
        return Err(frozen());
    }
    let mut files = BTreeMap::new();
    for row in &original {
        if row.kind == NamespaceEntryKind::Directory {
            if state.directory_git_repositories.contains_key(&row.entry_id) {
                return Err(Error::new(
                    ErrorCode::UnsupportedOperation,
                    "an initialized child repository must retain its independent lifecycle",
                ));
            }
        } else {
            files.entry(row.file_id.clone()).or_insert_with(|| row.path.clone());
        }
    }
    if files.len() > super::super::batch::MAX_FILES {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "directory deletion exceeds the complete atomic file frontier budget",
        ));
    }
    // Every ordinary ancestor remains the same reserved directory identity.
    let mut key = entry.lookup_key();
    while let Some((parent, _)) = key.rsplit_once('/') {
        if at(namespace, parent) != at(&head.entries, parent) {
            return Err(conflict("directory parent is not completely admitted"));
        }
        key = parent;
    }
    Ok(Captured {
        entry: entry.clone(),
        owner_directory_id: branch.directory_id.clone(),
        namespace_head: head.commit_id.clone(),
        namespace,
        branch: branch.directory_branch_id.clone(),
        tree: admitted.as_ref().map_or(branch.head_tree_id.as_str(), |scope| {
            scope.selected_tree_id()
        }).to_owned(),
        files,
    })
}

pub(super) fn inherits(state: &State, branch: &str, ancestor: &str) -> bool {
    let mut cursor = Some(branch);
    for _ in 0..state.directory_branches.len() {
        let Some(id) = cursor else { return false };
        if id == ancestor {
            return true;
        }
        cursor = state.directory_branches.get(id).and_then(|branch| branch.parent_branch_id.as_deref());
    }
    false
}

/// Rebase only an owner's exterior mount. Stable entry identity and all
/// relative paths inside the retained source tree must still match exactly.
pub(crate) fn retained_entries(
    node: &DirectoryDeletionNode,
    namespace: &crate::Commit,
    tree: &crate::DirectoryTree,
) -> Result<Vec<Entry>> {
    let owner_path = if node.owner_directory_id == node.repository_id {
        ""
    } else {
        namespace.entries.iter().find(|entry| {
            entry.entry_id == node.owner_directory_id
                && entry.kind == NamespaceEntryKind::Directory
        }).ok_or_else(invalid)?.path()
    };
    let source = tree.entries.iter().find(|entry| {
        entry.entry_id == node.directory_id && entry.kind == NamespaceEntryKind::Directory
    }).ok_or_else(invalid)?;
    let depth = if tree.path.is_empty() { 0 } else { tree.path.split('/').count() };
    let destination = if owner_path.is_empty() { None }
        else { Some(name_policy::PortablePath::parse(owner_path)?) };
    let rows = subtree(&tree.entries, source.lookup_key()).into_iter().map(|entry| {
        let location = match &destination {
            Some(destination) => entry.location().rebase(depth, destination)?,
            None => entry.location().strip_components(depth).ok_or_else(invalid)?,
        };
        let mut entry = entry.clone();
        entry.relocate(location);
        Ok(entry)
    }).collect::<Result<Vec<_>>>()?;
    if rows.first().is_none_or(|entry| entry.path != node.path) {
        return Err(invalid());
    }
    Ok(rows)
}

pub(super) fn retained<'a>(
    state: &'a State,
    node: &DirectoryDeletionNode,
) -> Result<(&'a crate::Commit, &'a Entry)> {
    let namespace = state.commits.get(&node.namespace_head).ok_or_else(invalid)?;
    let entry = at(&namespace.entries, node.path.key()).ok_or_else(invalid)?;
    let repository = state.repositories.get(&node.repository_id).ok_or_else(invalid)?;
    let tree = state.directory_trees.get(&node.source_tree_id).ok_or_else(invalid)?;
    let branch = state.directory_branches.get(&node.directory_branch_id).ok_or_else(invalid)?;
    if repository.history_tree_id != node.history_tree_id
        || namespace.history_tree_id != node.history_tree_id
        || entry.entry_id != node.directory_id
        || entry.kind != NamespaceEntryKind::Directory
        || entry.path != node.path
        || !inherits(state, &node.directory_branch_id, &tree.directory_branch_id)
        || branch.repository_id != node.repository_id
        || branch.directory_id != node.owner_directory_id
        || namespace.sequence >= node.sequence
        || tree.sequence >= node.sequence
        || subtree(&namespace.entries, node.path.key())
            != retained_entries(node, namespace, tree)?.iter().collect::<Vec<_>>()
    {
        return Err(invalid());
    }
    let files = subtree(&namespace.entries, node.path.key()).into_iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .map(|entry| entry.file_id.as_str()).collect::<BTreeSet<_>>();
    if files.into_iter().ne(node.files.keys().map(String::as_str)) {
        return Err(invalid());
    }
    Ok((namespace, entry))
}

pub(super) fn require_restore(
    state: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
    node: &DirectoryDeletionNode,
) -> Result<()> {
    let (original, _) = retained(state, node)?;
    let branch = selected(state, current, &node.repository_id, &node.directory_branch_id)?;
    if branch.directory_id != node.owner_directory_id {
        return Err(invalid());
    }
    let (_, retained, entries) = namespace(state, current, &node.repository_id)?;
    let expected = subtree(&original.entries, node.path.key());
    if subtree(entries, node.path.key()) != expected
        || subtree(&retained.entries, node.path.key()) != expected
    {
        return Err(conflict("deleted directory ownership or retained contents changed"));
    }
    let admitted = current.map(|view| view.batch_scope(state, &branch.directory_branch_id))
        .transpose()?;
    let scope = admitted.as_ref().map_or(branch, |scope| scope.branch());
    if subtree(&scope.entries, node.path.key()) != expected {
        return Err(conflict("selected directory composition differs from the deletion"));
    }
    let mut key = node.path.key();
    while let Some((parent, _)) = key.rsplit_once('/') {
        if at(entries, parent) != at(&original.entries, parent) {
            return Err(conflict("deleted directory's original parent identity changed"));
        }
        key = parent;
    }
    Ok(())
}

/// Capture the initialized owner before SMB persists its exact request. None
/// means this ordinary directory has no initialized owner; errors never grant
/// permission to fall back to a destructive namespace workspace.
pub(super) fn prepare(
    state: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
    repository: &str,
    path: &name_policy::LookupPath,
    expected_entry: &str,
    maximum: usize,
) -> Result<Option<DirectoryNamespaceBatch>> {
    let (_, _, entries) = namespace(state, current, repository)?;
    let entry = at(entries, path.key()).ok_or_else(|| {
        Error::new(ErrorCode::DirectoryNotFound, "directory is no longer present")
    })?;
    if entry.entry_id != expected_entry {
        return Err(Error::new(ErrorCode::WorkspaceChanged, "directory identity changed"));
    }
    if entry.kind != NamespaceEntryKind::Directory {
        return Err(Error::new(ErrorCode::NotDirectory, "rmdir requires a directory"));
    }
    if state.directory_git_repositories.contains_key(&entry.entry_id) {
        return Err(Error::new(
            ErrorCode::UnsupportedOperation,
            "an initialized repository cannot be deleted as an ordinary directory",
        ));
    }
    let mut parent = entry.lookup_key().rsplit_once('/').map(|(parent, _)| parent);
    let owner = loop {
        let Some(key) = parent else {
            break state.directory_git_repositories.get(repository);
        };
        let ancestor = at(entries, key).ok_or_else(invalid)?;
        if ancestor.kind != NamespaceEntryKind::Directory {
            return Err(invalid());
        }
        if let Some(owner) = state.directory_git_repositories.get(&ancestor.entry_id) {
            break Some(owner);
        }
        parent = key.rsplit_once('/').map(|(parent, _)| parent);
    };
    let Some(owner) = owner else { return Ok(None) };
    if owner.repository_id != repository {
        return Err(invalid());
    }
    let branch = state.directory_branches.get(&owner.current_branch_id).ok_or_else(invalid)?;
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Delete,
        items: vec![crate::DirectoryNamespaceItem {
            source: crate::DirectoryNamespaceSource {
                directory_branch_id: branch.directory_branch_id.clone(),
                expected_tree_id: branch.head_tree_id.clone(),
                path: entry.path().to_owned(),
                entry_id: entry.entry_id.clone(),
            },
            target: None,
        }],
    };
    capture(state, current, &DeleteInput {
        repository_id: repository.into(), request: request.clone(),
    }, maximum)?;
    Ok(Some(request))
}
