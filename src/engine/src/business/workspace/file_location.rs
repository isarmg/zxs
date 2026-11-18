//! A file workspace binds an explicit occurrence without changing content HEAD.
use crate::business::{WorkspaceInput, namespace};
use crate::engine::CurrentNamespaces;
use crate::{Entry, Error, NamespaceEntryKind, PathChangeError, Result, State};
use name_policy::PortablePath;
use std::borrow::Cow;

fn changed() -> Error {
    Error::PathChange(PathChangeError::WorkspaceLocation)
}

/// Preserve the admitted occurrence across unlocked upload I/O. Current names
/// are projected separately; an equal version alone cannot identify its owner.
/// Workspace/context admission remains responsible for branch and selection fences.
pub(super) fn same_source(left: Option<&Entry>, right: Option<&Entry>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            left.kind == right.kind
                && left.entry_id == right.entry_id
                && left.file_id == right.file_id
                && left.version_id == right.version_id
        }
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
    }
}

/// A namespace workspace may stage new nested directories, so absence on
/// *both* sides is valid. A new lookalike at an old parent path is not. These
/// are immutable, already admitted namespace snapshots, not mutable drafts.
pub(super) fn require_parents(
    base: namespace::NamespaceTree<'_>,
    current: namespace::NamespaceTree<'_>,
    key: &str,
) -> Result<()> {
    let mut component = key;
    // Include the leaf only when it was or is itself a directory. FILE content
    // and FILE selection conflicts are checked by their dedicated owner.
    let old_leaf = base.get_key(component);
    let new_leaf = current.get_key(component);
    if old_leaf.into_iter().chain(new_leaf).any(|entry| entry.kind == NamespaceEntryKind::Directory)
        && !same_directory(old_leaf, new_leaf)
    {
        return Err(changed());
    }
    while let Some((parent, _)) = component.rsplit_once('/') {
        if !same_directory(base.get_key(parent), current.get_key(parent)) {
            return Err(changed());
        }
        component = parent;
    }
    Ok(())
}

fn same_directory(before: Option<&Entry>, after: Option<&Entry>) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => {
            before.kind == NamespaceEntryKind::Directory
                && after.kind == NamespaceEntryKind::Directory
                && before.entry_id == after.entry_id
        }
        (None, None) => true,
        _ => false,
    }
}

pub(crate) fn project<'a>(
    state: &State,
    view: Option<&CurrentNamespaces>,
    workspace: &(impl WorkspaceInput + ?Sized),
    entries: &'a [Entry],
    path: &PortablePath,
) -> Result<Cow<'a, [Entry]>> {
    let branch = state.branches.get(workspace.branch_id()).ok_or_else(changed)?;
    let Some(file) = &branch.file_id else {
        return Ok(Cow::Borrowed(entries));
    };
    if workspace.files().any(|staged| staged.path().key() != path.key()) {
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
    let key = namespace::file_context::context_key_from_key(&repository.repository_id, path.key());
    let selected = workspace.selected_file_context(&key);
    let bound = workspace.selected_file_contexts()
        .any(|context| context.branch_id == workspace.branch_id());
    let Some(selected) = selected.filter(|context| context.branch_id == workspace.branch_id())
    else {
        // An unselected independent branch retains its own immutable namespace.
        // A selected workspace cannot silently fall back to a now-stale path.
        return if !bound && body.lookup_key() == path.key() {
            Ok(Cow::Borrowed(entries))
        } else {
            Err(changed())
        };
    };
    let main = state.branches.get(&repository.main_branch_id).ok_or_else(changed)?;
    let main_entries = match view {
        Some(view) => view.branch_entries(&repository.main_branch_id, &main.head_commit_id)?,
        None => &state.commits.get(&main.head_commit_id).ok_or_else(changed)?.entries,
    };
    let namespace = namespace::NamespaceTree::from_current_state(
        state, &repository.repository_id, main_entries,
    );
    let current = namespace.get_key(path.key()).ok_or_else(changed)?;
    let context = namespace::file_context::context_at_key(state, &repository.repository_id, path.key())
        .ok_or_else(changed)?;
    if &context != selected
        || !same_source(Some(current), Some(body))
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
        projected.push(entry.clone());
        key = parent;
    }
    namespace::sorted(&mut projected);
    Ok(Cow::Owned(projected))
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/business/workspace_file_location_tests.rs"]
mod tests;
