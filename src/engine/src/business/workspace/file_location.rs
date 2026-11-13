//! A file workspace binds an explicit occurrence without changing content HEAD.
use crate::business::{directory, namespace};
use crate::engine::CurrentNamespaces;
use crate::{Entry, Error, NamespaceEntryKind, PathChangeError, Result, State, Workspace};
use name_policy::PortablePath;
use std::borrow::Cow;

fn changed() -> Error {
    Error::PathChange(PathChangeError::WorkspaceLocation)
}

pub(crate) fn project<'a>(
    state: &State,
    view: Option<&CurrentNamespaces>,
    workspace: &Workspace,
    entries: &'a [Entry],
    path: &PortablePath,
) -> Result<Cow<'a, [Entry]>> {
    let branch = state.branches.get(&workspace.branch_id).ok_or_else(changed)?;
    let Some(file) = &branch.file_id else {
        return Ok(Cow::Borrowed(entries));
    };
    if workspace.files.values().any(|staged| staged.path.key() != path.key()) {
        return Err(changed());
    }
    let mut bodies = entries.iter().filter(|entry| entry.kind == NamespaceEntryKind::File);
    let body = bodies.next().filter(|entry| &entry.file_id == file).ok_or_else(changed)?;
    if bodies.next().is_some() {
        return Err(changed());
    }
    let repository = state.repositories.values()
        .find(|repository| repository.history_tree_id == branch.history_tree_id)
        .ok_or_else(changed)?;
    let key = directory::context_key_from_key(&repository.repository_id, path.key());
    let selected = workspace.selected_file_contexts.get(&key);
    let bound = workspace.selected_file_contexts.values()
        .any(|context| context.branch_id == workspace.branch_id);
    let Some(selected) = selected.filter(|context| context.branch_id == workspace.branch_id) else {
        // An unselected independent branch retains its own immutable namespace.
        // A selected workspace cannot silently fall back to a now-stale path.
        return if !bound && body.lookup_key() == path.key() {
            Ok(Cow::Borrowed(entries))
        } else {
            Err(changed())
        };
    };
    let changed_selection = |id: &str| {
        state.directory_selection_generations.get(id).is_some_and(|generation| {
            workspace.directory_selection_generations.get(id).copied().unwrap_or(0) != *generation
        })
    };
    if changed_selection(&repository.repository_id) {
        return Err(changed());
    }
    let main = state.branches.get(&repository.main_branch_id).ok_or_else(changed)?;
    let main_entries = match view {
        Some(view) => view.branch_entries(&repository.main_branch_id, &main.head_commit_id)?,
        None => &state.commits.get(&main.head_commit_id).ok_or_else(changed)?.entries,
    };
    let namespace = namespace::NamespaceTree::from_current_state(
        state, &repository.repository_id, main_entries,
    );
    let current = namespace.get_key(path.key()).ok_or_else(changed)?;
    let context = directory::context_at_key(state, &repository.repository_id, path.key())
        .ok_or_else(changed)?;
    if &context != selected
        || current.kind != NamespaceEntryKind::File
        || current.file_id != body.file_id
        || current.entry_id != body.entry_id
        || current.version_id != body.version_id
        || current.size != body.size
    {
        return Err(changed());
    }
    let mut projected = vec![current.clone()];
    let mut key = current.lookup_key();
    while let Some((parent, _)) = key.rsplit_once('/') {
        let entry = namespace.get_key(parent)
            .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
            .ok_or_else(changed)?;
        if changed_selection(&entry.entry_id) {
            return Err(changed());
        }
        projected.push(entry.clone());
        key = parent;
    }
    namespace::sorted(&mut projected);
    Ok(Cow::Owned(projected))
}
