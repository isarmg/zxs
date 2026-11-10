//! Batch inputs bind the selected tree to exact current owner occurrences.
//! The named branch head remains the compare-and-swap baseline, not the data
//! source when an admitted directory selection published a separate projection.
use super::directory_branches::{descendants, inherits, rebase, validate_context};
use super::*;
use crate::{DirectoryBranch, DirectoryTree};

pub(crate) struct CurrentBatchScope {
    branch: DirectoryBranch,
    selected_tree: String,
}

impl CurrentBatchScope {
    pub(crate) fn branch(&self) -> &DirectoryBranch {
        &self.branch
    }

    pub(crate) fn selected_tree_id(&self) -> &str {
        &self.selected_tree
    }
}

/// Reuse consecutive items' exact admitted source, bound to one immutable
/// input. Switching branches releases it before capturing another, so a batch
/// retains at most one scope. Per-item HEAD and occurrence checks stay separate.
pub(crate) struct CurrentBatchSources<'a> {
    state: &'a State,
    current: &'a CurrentNamespaces,
    scope: Option<CurrentBatchScope>,
}

impl CurrentBatchSources<'_> {
    pub(crate) fn get(&mut self, id: &str) -> Result<&CurrentBatchScope> {
        if self
            .scope
            .as_ref()
            .is_none_or(|scope| scope.branch.directory_branch_id != id)
        {
            self.scope = None;
            self.scope = Some(self.current.batch_scope(self.state, id)?);
        }
        Ok(self
            .scope
            .as_ref()
            .expect("invariant: the requested source was admitted"))
    }
}

fn unsupported_selection() -> Error {
    Error::new(
        ErrorCode::RecoveryRequired,
        "batch source is not the selected current directory composition",
    )
}

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::WorkspaceChanged, message)
}

/// Selection IDs locate a tree; stable directory ownership and branch ancestry
/// prove which namespace it can describe. A same-name replacement is unrelated.
fn selected_tree<'a>(
    state: &'a State,
    repository: &str,
    directory: &str,
    branch_id: &str,
    tree_id: &str,
) -> Result<&'a DirectoryTree> {
    if state
        .directory_selections
        .get(directory)
        .map(String::as_str)
        != Some(branch_id)
        || state
            .directory_tree_selections
            .get(directory)
            .map(String::as_str)
            != Some(tree_id)
    {
        return Err(unsupported_selection());
    }
    let branch = state.directory_branches.get(branch_id).ok_or_else(stale)?;
    let tree = state.directory_trees.get(tree_id).ok_or_else(stale)?;
    let owner = state
        .directory_branches
        .get(&tree.directory_branch_id)
        .ok_or_else(stale)?;
    let history = &state
        .repositories
        .get(repository)
        .ok_or_else(stale)?
        .history_tree_id;
    if branch.repository_id != repository
        || owner.repository_id != repository
        || branch.directory_id != directory
        || owner.directory_id != directory
        || !inherits(state, branch_id, &tree.directory_branch_id)
        || state
            .commits
            .get(&tree.commit_id)
            .map(|commit| &commit.history_tree_id)
            != Some(history)
    {
        return Err(invalid(
            "selected batch tree escapes its directory identity or history",
        ));
    }
    Ok(tree)
}

fn validate_tree_contexts(state: &State, repository: &str, tree: &DirectoryTree) -> Result<()> {
    if descendants(&tree.entries, &name_policy::path_key(&tree.path)).len() != tree.entries.len() {
        return Err(invalid(
            "selected batch tree contains occurrences outside its own path",
        ));
    }
    let mut paths = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut directories = BTreeSet::new();
    let history = &state
        .repositories
        .get(repository)
        .ok_or_else(stale)?
        .history_tree_id;
    for entry in &tree.entries {
        match entry.kind {
            NamespaceEntryKind::File => {
                paths.insert(entry.lookup_key());
                files.insert(entry.file_id.as_str());
                let context = tree
                    .file_contexts
                    .get(entry.lookup_key())
                    .ok_or_else(stale)?;
                validate_context(state, repository, entry, context)?;
                let id = tree.file_branches.get(&entry.file_id).ok_or_else(stale)?;
                if !state.branches.get(id).is_some_and(|branch| {
                    branch.state == BranchState::Active
                        && branch.history_tree_id == *history
                        && branch.file_id.as_deref() == Some(entry.file_id.as_str())
                }) {
                    return Err(invalid("selected batch tree has an unrelated file branch"));
                }
            }
            NamespaceEntryKind::Directory => {
                directories.insert(entry.entry_id.as_str());
                match (
                    state.directory_selections.get(&entry.entry_id),
                    tree.directory_refs.get(&entry.entry_id),
                ) {
                    (None, None) => {}
                    (Some(id), Some(reference)) if *id == reference.directory_branch_id => {}
                    _ => {
                        return Err(invalid(
                            "selected batch tree omits or replaces a descendant selection",
                        ));
                    }
                }
            }
        }
    }
    if tree.file_contexts.len() != paths.len()
        || tree.file_branches.len() != files.len()
        || tree.inherited_files.iter().any(|(file, origin)| {
            !files.contains(file.as_str())
                || !state
                    .directory_branches
                    .get(origin)
                    .is_some_and(|branch| branch.repository_id == repository)
        })
        || tree
            .directory_refs
            .keys()
            .any(|id| !directories.contains(id.as_str()))
    {
        return Err(invalid(
            "selected batch tree retains metadata outside its occurrences",
        ));
    }
    Ok(())
}

/// Directory refs form a flattened descendant graph. Verify every node against
/// this selected root's occurrences, not a descendant branch's unrelated head.
fn validate_selected_graph(
    state: &State,
    branch: &DirectoryBranch,
    tree: &DirectoryTree,
) -> Result<()> {
    validate_tree_contexts(state, &branch.repository_id, tree)?;
    let directories = tree
        .entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
        .map(|entry| (entry.entry_id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    for (id, reference) in &tree.directory_refs {
        let entry = directories[id.as_str()];
        let child = selected_tree(
            state,
            &branch.repository_id,
            id,
            &reference.directory_branch_id,
            &reference.tree_id,
        )?;
        validate_tree_contexts(state, &branch.repository_id, child)?;
        let expected = rebase(
            descendants(&tree.entries, entry.lookup_key()),
            entry.path(),
            &child.path,
        )?;
        if child.entries != expected
            || child
                .directory_refs
                .iter()
                .any(|(id, reference)| tree.directory_refs.get(id) != Some(reference))
        {
            return Err(invalid(
                "selected batch descendant differs from its exact parent occurrences or refs",
            ));
        }
    }
    Ok(())
}

/// A selected ancestor can supply inherited write intent to a child tree without
/// advancing that child's head. Capture the actual owner's path context after
/// proving the entire occurrence, rather than reviving the child's frozen intent
/// or borrowing the context of another alias with the same file ID.
fn current_contexts(state: &State, branch: &mut DirectoryBranch) -> Result<()> {
    branch.file_contexts.clear();
    branch.file_branches.clear();
    branch.inherited_files.clear();
    let repository = state
        .repositories
        .get(&branch.repository_id)
        .ok_or_else(stale)?;
    for entry in branch
        .entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
    {
        let context = state
            .file_contexts
            .get(&crate::business::context_key_from_key(
                &branch.repository_id,
                entry.lookup_key(),
            ))
            .ok_or_else(stale)?;
        validate_context(state, &branch.repository_id, entry, context)?;
        let selected = state
            .file_selections
            .get(&entry.file_id)
            .ok_or_else(stale)?;
        if !state.branches.get(selected).is_some_and(|selected| {
            selected.state == BranchState::Active
                && selected.history_tree_id == repository.history_tree_id
                && selected.file_id.as_deref() == Some(entry.file_id.as_str())
        }) {
            return Err(invalid(
                "current batch occurrence has an unrelated selected file branch",
            ));
        }
        branch
            .file_contexts
            .insert(entry.lookup_key().into(), context.clone());
        branch
            .file_branches
            .insert(entry.file_id.clone(), selected.clone());
        if let Some(origin) = state.file_write_origins.get(&entry.file_id) {
            branch
                .inherited_files
                .insert(entry.file_id.clone(), origin.clone());
        }
    }
    Ok(())
}

impl CurrentNamespaces {
    pub(crate) fn batch_sources<'a>(&'a self, state: &'a State) -> CurrentBatchSources<'a> {
        CurrentBatchSources {
            state,
            current: self,
            scope: None,
        }
    }

    /// The returned type is usable only with this mutation's immutable input.
    /// Expected-head checks remain against State before this source is captured.
    pub(crate) fn batch_scope(&self, state: &State, id: &str) -> Result<CurrentBatchScope> {
        let branch = state.directory_branches.get(id).ok_or_else(stale)?;
        let repository = state
            .repositories
            .get(&branch.repository_id)
            .ok_or_else(stale)?;
        let owner = self.writable_scope(state, &repository.main_branch_id)?;
        if !self.directory_writable(state, id)? {
            return Err(Error::new(
                ErrorCode::BranchNotActive,
                "batch scope has no retained writable registration",
            ));
        }
        if state
            .directory_selections
            .get(&branch.directory_id)
            .map(String::as_str)
            != Some(id)
        {
            return Err(unsupported_selection());
        }
        let selected = state
            .directory_tree_selections
            .get(&branch.directory_id)
            .ok_or_else(stale)?;
        let tree = selected_tree(
            state,
            &branch.repository_id,
            &branch.directory_id,
            id,
            selected,
        )?;
        let mut source = if selected == &branch.head_tree_id {
            // D3 can filter an existing selected head without materializing a
            // selection tree first. Reuse the already admitted head projection.
            let projection = self.directory_projection(state, id)?;
            let source = projection.branch().clone();
            for (directory, reference) in &source.directory_refs {
                selected_tree(
                    state,
                    &branch.repository_id,
                    directory,
                    &reference.directory_branch_id,
                    &reference.tree_id,
                )?;
            }
            source
        } else {
            validate_selected_graph(state, branch, tree)?;
            let mut source = branch.clone();
            source.commit_id = tree.commit_id.clone();
            source.path = tree.path.clone();
            source.entries = tree.entries.clone();
            source.directory_refs = tree.directory_refs.clone();
            source
        };
        let path = if branch.directory_id == repository.repository_id {
            ""
        } else {
            owner
                .entries
                .iter()
                .find(|entry| {
                    entry.kind == NamespaceEntryKind::Directory
                        && entry.entry_id == branch.directory_id
                })
                .map(Entry::path)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::DirectoryBranchConflict,
                        "batch directory identity is absent from the current owner",
                    )
                })?
        };
        let restored = rebase(&source.entries, &source.path, path)?;
        if restored != descendants(&owner.entries, &name_policy::path_key(path)) {
            return Err(if selected == &branch.head_tree_id {
                invalid(
                    "selected batch composition differs from its exact current owner occurrences",
                )
            } else {
                unsupported_selection()
            });
        }
        source.path = path.into();
        source.entries = restored;
        current_contexts(state, &mut source)?;
        Ok(CurrentBatchScope {
            branch: source,
            selected_tree: selected.clone(),
        })
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/current_selected_batch_tests.rs"]
mod tests;
