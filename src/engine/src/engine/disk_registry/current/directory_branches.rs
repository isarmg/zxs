//! Current directory choices bind an admitted head to one live directory identity.
//! Frozen trees remain historical evidence. A selected projection publishes new
//! immutable trees only where its entries, contexts or descendant refs differ.
use super::*;
use crate::{DirectoryBranch, DirectoryTree, FileContext};

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::WorkspaceChanged, message)
}

fn at_key<'a>(entries: &'a [Entry], key: &str) -> Option<&'a Entry> {
    entries
        .binary_search_by(|entry| entry.lookup_key().cmp(key))
        .ok()
        .map(|index| &entries[index])
}

pub(super) fn descendants<'a>(entries: &'a [Entry], key: &str) -> &'a [Entry] {
    if key.is_empty() {
        return entries;
    }
    let prefix = format!("{key}/");
    let start = entries.partition_point(|entry| entry.lookup_key() < prefix.as_str());
    let entries = &entries[start..];
    let end = entries.partition_point(|entry| entry.lookup_key().starts_with(&prefix));
    &entries[..end]
}

pub(super) fn rebase(entries: &[Entry], from: &str, to: &str) -> Result<Vec<Entry>> {
    let depth = if from.is_empty() {
        0
    } else {
        from.split('/').count()
    };
    let target = (!to.is_empty())
        .then(|| name_policy::PortablePath::parse(to))
        .transpose()?;
    entries
        .iter()
        .map(|entry| {
            let mut entry = entry.clone();
            let path = match &target {
                Some(target) => entry.location().rebase(depth, target)?,
                None => entry
                    .location()
                    .strip_components(depth)
                    .ok_or_else(|| invalid("directory projection contains its own root"))?,
            };
            entry.relocate(path);
            Ok(entry)
        })
        .collect()
}

fn current_path<'a>(owner: &'a [Entry], repository: &str, directory: &str) -> Result<&'a str> {
    if directory == repository {
        return Ok("");
    }
    owner
        .iter()
        .find(|entry| entry.kind == NamespaceEntryKind::Directory && entry.entry_id == directory)
        .map(Entry::path)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::DirectoryNotFound,
                "directory identity is absent from the current namespace",
            )
        })
}

pub(super) fn inherits(state: &State, branch: &str, ancestor: &str) -> bool {
    let mut current = Some(branch);
    let mut seen = BTreeSet::new();
    while let Some(id) = current {
        if id == ancestor {
            return true;
        }
        if !seen.insert(id) {
            return false;
        }
        current = state
            .directory_branches
            .get(id)
            .and_then(|branch| branch.parent_branch_id.as_deref());
    }
    false
}

pub(super) fn validate_context(
    state: &State,
    repository: &str,
    entry: &Entry,
    context: &FileContext,
) -> Result<()> {
    let history = &state
        .repositories
        .get(repository)
        .ok_or_else(stale)?
        .history_tree_id;
    let branch = state.branches.get(&context.branch_id).ok_or_else(stale)?;
    if branch.state != BranchState::Active
        || branch.history_tree_id != *history
        || branch.file_id.as_deref() != Some(entry.file_id.as_str())
        || context.inherited_from.as_ref().is_some_and(|id| {
            !state
                .directory_branches
                .get(id)
                .is_some_and(|branch| branch.repository_id == repository)
        })
    {
        return Err(invalid(
            "directory context escapes its admitted file or history",
        ));
    }
    if let Some(node) = &context.node_id
        && crate::business::file_nodes::node_version(state, &entry.file_id, node)?
            != entry.version_id
    {
        return Err(invalid(
            "directory context node differs from its admitted occurrence",
        ));
    }
    Ok(())
}

/// Trim only references whose exact occurrences were removed from this source.
/// A retained file never borrows another path's context or global file selection.
fn project_branch(
    state: &State,
    source: &DirectoryBranch,
    entries: &[Entry],
) -> Result<DirectoryBranch> {
    let mut branch = DirectoryBranch {
        directory_branch_id: source.directory_branch_id.clone(),
        repository_id: source.repository_id.clone(),
        directory_id: source.directory_id.clone(),
        path: source.path.clone(),
        name: source.name.clone(),
        parent_branch_id: source.parent_branch_id.clone(),
        head_tree_id: source.head_tree_id.clone(),
        commit_id: source.commit_id.clone(),
        entries: entries.to_vec(),
        file_contexts: BTreeMap::new(),
        file_branches: BTreeMap::new(),
        inherited_files: BTreeMap::new(),
        directory_refs: BTreeMap::new(),
        materialized: source.materialized,
        sequence: source.sequence,
    };
    for entry in entries {
        if at_key(&source.entries, entry.lookup_key()) != Some(entry) {
            return Err(invalid(
                "directory projection imports an occurrence outside its source",
            ));
        }
        match entry.kind {
            NamespaceEntryKind::File => {
                let context = source
                    .file_contexts
                    .get(entry.lookup_key())
                    .ok_or_else(stale)?;
                validate_context(state, &source.repository_id, entry, context)?;
                let file_branch = source.file_branches.get(&entry.file_id).ok_or_else(stale)?;
                let record = state.branches.get(file_branch).ok_or_else(stale)?;
                if record.file_id.as_deref() != Some(entry.file_id.as_str())
                    || record.state != BranchState::Active
                    || record.history_tree_id
                        != state.repositories[&source.repository_id].history_tree_id
                {
                    return Err(invalid(
                        "directory source file branch differs from its occurrence",
                    ));
                }
                branch
                    .file_contexts
                    .insert(entry.lookup_key().into(), context.clone());
                branch
                    .file_branches
                    .insert(entry.file_id.clone(), file_branch.clone());
                if let Some(origin) = source.inherited_files.get(&entry.file_id) {
                    branch
                        .inherited_files
                        .insert(entry.file_id.clone(), origin.clone());
                }
            }
            NamespaceEntryKind::Directory => {
                if let Some(reference) = source.directory_refs.get(&entry.entry_id) {
                    branch
                        .directory_refs
                        .insert(entry.entry_id.clone(), reference.clone());
                }
            }
        }
    }
    Ok(branch)
}

fn tree_from_branch(branch: &DirectoryBranch, source: &DirectoryTree) -> DirectoryTree {
    DirectoryTree {
        tree_id: source.tree_id.clone(),
        directory_branch_id: branch.directory_branch_id.clone(),
        parent_tree_id: source.parent_tree_id.clone(),
        commit_id: branch.commit_id.clone(),
        path: branch.path.clone(),
        entries: branch.entries.clone(),
        file_contexts: branch.file_contexts.clone(),
        file_branches: branch.file_branches.clone(),
        inherited_files: branch.inherited_files.clone(),
        directory_refs: branch.directory_refs.clone(),
        sequence: source.sequence,
    }
}

struct ProjectedTree {
    source: Arc<DirectoryTree>,
    projected: DirectoryTree,
    selected_branch: String,
}

/// Created only from a head present in this registry's admitted directory map.
/// All descendant nodes consume the same root occurrence projection, not their
/// unrelated current heads and not an arbitrary historical tree lookup.
pub(crate) struct CurrentDirectoryProjection {
    branch: DirectoryBranch,
    path: String,
    nodes: BTreeMap<String, ProjectedTree>,
}

impl CurrentNamespaces {
    pub(crate) fn require_directory_branch_write(&self, state: &State, id: &str) -> Result<()> {
        let branch = state.directory_branches.get(id).ok_or_else(stale)?;
        let repo = state
            .repositories
            .get(&branch.repository_id)
            .ok_or_else(stale)?;
        self.require_branch_write(state, &repo.main_branch_id)?;
        if !self.directory_writable(state, id)? {
            return Err(Error::new(
                ErrorCode::BranchNotActive,
                "directory source has no retained writable registration",
            ));
        }
        Ok(())
    }

    pub(crate) fn directory_projection(
        &self,
        state: &State,
        id: &str,
    ) -> Result<CurrentDirectoryProjection> {
        let source = state.directory_branches.get(id).ok_or_else(stale)?;
        let repo = state
            .repositories
            .get(&source.repository_id)
            .ok_or_else(stale)?;
        let owner = state.branches.get(&repo.main_branch_id).ok_or_else(stale)?;
        let owner = self.branch_entries(&repo.main_branch_id, &owner.head_commit_id)?;
        let path = current_path(owner, &repo.repository_id, &source.directory_id)?.to_owned();
        let entries = self.directory_entries(id, &source.head_tree_id)?;
        let branch = project_branch(state, source, entries)?;
        let original = state
            .directory_trees
            .get_shared(&source.head_tree_id)
            .ok_or_else(stale)?;
        let tree_owner = state
            .directory_branches
            .get(&original.directory_branch_id)
            .ok_or_else(stale)?;
        if original.entries != source.entries
            || original.commit_id != source.commit_id
            || original.path != source.path
            || original.file_branches != source.file_branches
            || original.directory_refs != source.directory_refs
            || tree_owner.repository_id != source.repository_id
            || tree_owner.directory_id != source.directory_id
            || !inherits(state, id, &original.directory_branch_id)
        {
            return Err(invalid("directory head differs from its frozen source"));
        }
        if source.materialized {
            if original.directory_branch_id != source.directory_branch_id
                || original.file_contexts != source.file_contexts
                || original.inherited_files != source.inherited_files
                || original.sequence != source.sequence
            {
                return Err(invalid(
                    "materialized directory contexts differ from its immutable head",
                ));
            }
        } else if source.parent_branch_id.is_none()
            || source.file_contexts != inherited_contexts(&original, id)
            || source.inherited_files
                != original
                    .file_branches
                    .keys()
                    .map(|file| (file.clone(), id.to_owned()))
                    .collect()
        {
            return Err(invalid(
                "unmaterialized directory does not inherit its exact frozen contexts",
            ));
        }
        let mut nodes = BTreeMap::new();
        let directories = branch
            .entries
            .iter()
            .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
            .map(|entry| (entry.entry_id.as_str(), entry))
            .collect::<BTreeMap<_, _>>();
        for (directory, reference) in &branch.directory_refs {
            let entry = directories
                .get(directory.as_str())
                .expect("invariant: projected references come from retained directory entries");
            let selected = state
                .directory_branches
                .get(&reference.directory_branch_id)
                .ok_or_else(stale)?;
            let tree = state
                .directory_trees
                .get_shared(&reference.tree_id)
                .ok_or_else(stale)?;
            let owner = state
                .directory_branches
                .get(&tree.directory_branch_id)
                .ok_or_else(stale)?;
            if selected.directory_id != *directory
                || owner.directory_id != *directory
                || selected.repository_id != source.repository_id
                || owner.repository_id != source.repository_id
                || !inherits(
                    state,
                    &reference.directory_branch_id,
                    &tree.directory_branch_id,
                )
            {
                return Err(invalid(
                    "directory descendant reference escapes its source identity",
                ));
            }
            let raw = rebase(
                descendants(&source.entries, entry.lookup_key()),
                entry.path(),
                &tree.path,
            )?;
            if raw != tree.entries {
                return Err(invalid(
                    "directory descendant tree differs from its frozen parent occurrences",
                ));
            }
            let kept = rebase(
                descendants(entries, entry.lookup_key()),
                entry.path(),
                &tree.path,
            )?;
            let mut snapshot = owner.clone();
            snapshot.commit_id = tree.commit_id.clone();
            snapshot.path = tree.path.clone();
            snapshot.entries = tree.entries.clone();
            snapshot.file_contexts = tree.file_contexts.clone();
            snapshot.file_branches = tree.file_branches.clone();
            snapshot.inherited_files = tree.inherited_files.clone();
            snapshot.directory_refs = tree.directory_refs.clone();
            let snapshot = project_branch(state, &snapshot, &kept)?;
            for (child, reference) in &snapshot.directory_refs {
                if branch.directory_refs.get(child) != Some(reference) {
                    return Err(invalid(
                        "directory descendant graph differs from its admitted root",
                    ));
                }
            }
            nodes.insert(
                directory.clone(),
                ProjectedTree {
                    projected: tree_from_branch(&snapshot, &tree),
                    source: tree,
                    selected_branch: reference.directory_branch_id.clone(),
                },
            );
        }
        nodes.insert(
            branch.directory_id.clone(),
            ProjectedTree {
                projected: tree_from_branch(&branch, &original),
                source: original,
                selected_branch: id.into(),
            },
        );
        Ok(CurrentDirectoryProjection {
            branch,
            path,
            nodes,
        })
    }
}

impl CurrentDirectoryProjection {
    pub(crate) fn branch(&self) -> &DirectoryBranch {
        &self.branch
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    /// Exact frozen nodes from this admitted head's retained occurrence graph.
    /// A metadata publication must not substitute a descendant's newer HEAD.
    pub(super) fn file_tag_trees(&self) -> impl Iterator<Item = (&str, &DirectoryTree)> {
        self.nodes
            .iter()
            .map(|(directory, node)| (directory.as_str(), &node.projected))
    }

    fn matching_selected_tree(
        &self,
        state: &State,
        directory: &str,
    ) -> Result<Option<DirectoryTree>> {
        let node = &self.nodes[directory];
        if state.directory_selections.get(directory) != Some(&node.selected_branch) {
            return Ok(None);
        }
        let id = state
            .directory_tree_selections
            .get(directory)
            .ok_or_else(stale)?;
        let candidate = state.directory_trees.get(id).ok_or_else(stale)?;
        let mut expected = node.projected.clone();
        for (child, reference) in &mut expected.directory_refs {
            reference.tree_id = state
                .directory_tree_selections
                .get(child)
                .ok_or_else(stale)?
                .clone();
        }
        // Identity/lineage are checked separately during publication. Read-only
        // no-change detection compares all contents, including the full ref graph.
        expected.tree_id = candidate.tree_id.clone();
        expected.parent_tree_id = candidate.parent_tree_id.clone();
        expected.sequence = candidate.sequence;
        if expected != *candidate {
            return Ok(None);
        }
        Ok(Some(expected))
    }

    pub(crate) fn matches_selection(&self, state: &State) -> Result<bool> {
        for directory in self.nodes.keys() {
            if self.matching_selected_tree(state, directory)?.is_none() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn install_selection(&self, state: &mut State) -> Result<()> {
        let mut nodes: Vec<_> = self.nodes.iter().collect();
        // Every node's ref set is a strict descendant subset of the root graph.
        // Historical paths can have different depths after an earlier rename.
        nodes.sort_by_key(|(_, node)| node.projected.directory_refs.len());
        for (directory, node) in nodes {
            let mut tree = node.projected.clone();
            for (child, reference) in &mut tree.directory_refs {
                reference.tree_id = state
                    .directory_tree_selections
                    .get(child)
                    .ok_or_else(stale)?
                    .clone();
            }
            if tree != *node.source {
                tree.tree_id = crate::new_id("dtree")?;
                tree.parent_tree_id = Some(node.source.tree_id.clone());
                tree.sequence = state.sequence;
                state
                    .directory_trees
                    .insert(tree.tree_id.clone(), tree.clone());
            }
            state
                .directory_selections
                .insert(directory.clone(), node.selected_branch.clone());
            state
                .directory_tree_selections
                .insert(directory.clone(), tree.tree_id);
        }
        Ok(())
    }

    fn verify_selection(&self, previous: &State, next: &State) -> Result<()> {
        for (directory, node) in &self.nodes {
            if next.directory_trees.get(&node.source.tree_id) != Some(node.source.as_ref()) {
                return Err(invalid(
                    "directory selection rewrites a historical source tree",
                ));
            }
            let tree = self
                .matching_selected_tree(next, directory)?
                .ok_or_else(|| {
                    invalid("selected directory graph differs from the admitted projection")
                })?;
            if tree.tree_id == node.source.tree_id {
                if tree != *node.source {
                    return Err(invalid("directory selection changes its historical source"));
                }
            } else if previous.directory_trees.contains_key(&tree.tree_id)
                || tree.parent_tree_id.as_deref() != Some(node.source.tree_id.as_str())
                || tree.sequence != next.sequence
            {
                return Err(invalid(
                    "directory selection tree lacks fresh source-bound lineage",
                ));
            }
        }
        Ok(())
    }
}

impl CurrentNamespaces {
    /// Reconstruct the source from the authenticated previous State during both
    /// live publication and replay. Receipt fields locate it; they never prove it.
    pub(super) fn directory_selection_source(
        &self,
        previous: &State,
        next: &State,
        branch: &crate::Branch,
        commit: &Commit,
    ) -> Result<Option<CurrentScope>> {
        if commit.kind != "directory_selection" {
            return Ok(None);
        }
        let receipt = next
            .operations
            .get(&commit.operation_id)
            .ok_or_else(stale)?;
        if !matches!(
            receipt.kind.as_str(),
            "directory.branches.select" | "directory.default.select" | "directory.default.set"
        ) || receipt.commit_id.as_deref() != Some(commit.commit_id.as_str())
            || receipt.sequence != next.sequence
        {
            return Err(invalid(
                "directory selection has no exact operation binding",
            ));
        }
        let id = receipt
            .result
            .get("directory_branch_id")
            .and_then(|id| id.as_str())
            .ok_or_else(stale)?;
        self.require_directory_branch_write(previous, id)?;
        let projection = self.directory_projection(previous, id)?;
        let selected = projection.branch();
        let repo = previous
            .repositories
            .get(&selected.repository_id)
            .ok_or_else(stale)?;
        let old = self.branches.get(&repo.main_branch_id).ok_or_else(stale)?;
        let mut expected_branch = previous
            .branches
            .get(&repo.main_branch_id)
            .ok_or_else(stale)?
            .clone();
        expected_branch.head_commit_id = commit.commit_id.clone();
        expected_branch.generation = expected_branch
            .generation
            .checked_add(1)
            .ok_or_else(stale)?;
        if repo.main_branch_id != branch.branch_id
            || branch != &expected_branch
            || next.repositories.get(&repo.repository_id) != Some(repo)
            || next.directory_branches.get(id) != previous.directory_branches.get(id)
            || commit.history_tree_id != repo.history_tree_id
            || commit.parent_commit_ids != [old.head.clone()]
            || commit.provenance.as_deref() != Some(selected.commit_id.as_str())
            || commit.sequence != next.sequence
            || next.commits.get(&old.head) != previous.commits.get(&old.head)
        {
            return Err(invalid(
                "directory selection changed its admitted owner or source",
            ));
        }
        let path = projection.path();
        let key = name_policy::path_key(path);
        let restored = rebase(&selected.entries, &selected.path, path)?;
        let mut expected = old.entries.as_ref().clone();
        let prefix = format!("{key}/");
        expected.retain(|entry| !key.is_empty() && !entry.lookup_key().starts_with(&prefix));
        expected.extend(restored.iter().cloned());
        expected.sort_by(|a, b| a.lookup_key().cmp(b.lookup_key()));
        if commit.entries != expected {
            return Err(invalid(
                "directory selection differs from the exact current-source splice",
            ));
        }
        for (source, target) in selected
            .entries
            .iter()
            .zip(&restored)
            .filter(|(_, entry)| entry.kind == NamespaceEntryKind::File)
        {
            let context = selected
                .file_contexts
                .get(source.lookup_key())
                .ok_or_else(stale)?;
            if next
                .file_contexts
                .get(&crate::business::context_key_from_key(
                    &repo.repository_id,
                    target.lookup_key(),
                ))
                != Some(context)
            {
                return Err(invalid(
                    "directory selection context differs from its source occurrence",
                ));
            }
        }
        for entry in descendants(&old.entries, &key) {
            if entry.kind == NamespaceEntryKind::File
                && !at_key(&expected, entry.lookup_key())
                    .is_some_and(|entry| entry.kind == NamespaceEntryKind::File)
                && next
                    .file_contexts
                    .contains_key(&crate::business::context_key_from_key(
                        &repo.repository_id,
                        entry.lookup_key(),
                    ))
            {
                return Err(invalid(
                    "directory selection retains a removed path context",
                ));
            }
            if entry.kind == NamespaceEntryKind::Directory
                && !projection.nodes.contains_key(&entry.entry_id)
                && (next.directory_selections.contains_key(&entry.entry_id)
                    || next.directory_tree_selections.contains_key(&entry.entry_id))
            {
                return Err(invalid(
                    "directory selection retains a descendant outside its source graph",
                ));
            }
        }
        for hidden in self.removed_paths(&repo.main_branch_id) {
            if !at_key(&expected, hidden)
                .is_some_and(|entry| entry.kind == NamespaceEntryKind::File)
                && next
                    .file_contexts
                    .contains_key(&crate::business::context_key_from_key(
                        &repo.repository_id,
                        hidden,
                    ))
            {
                return Err(invalid(
                    "directory selection restores a hidden path context",
                ));
            }
        }
        for (file, source_branch) in &selected.file_branches {
            if next.file_selections.get(file) != Some(source_branch)
                || next.file_write_origins.get(file) != selected.inherited_files.get(file)
            {
                return Err(invalid(
                    "directory selection file references differ from its projected source",
                ));
            }
        }
        projection.verify_selection(previous, next)?;
        let mut admitted = old.as_ref().clone();
        admitted.entries = Arc::new(expected);
        Ok(Some(admitted))
    }

    /// A new unselected fork can borrow only the freshly captured current owner
    /// composition, never another branch's globally retained identities.
    pub(super) fn directory_fork_source(
        &self,
        previous: &State,
        next: &State,
        branch: &DirectoryBranch,
        owner: &CurrentScope,
    ) -> Result<Option<CurrentScope>> {
        if previous
            .directory_branches
            .contains_key(&branch.directory_branch_id)
        {
            return Ok(None);
        }
        let repo = next
            .repositories
            .get(&branch.repository_id)
            .ok_or_else(stale)?;
        let path = current_path(&owner.entries, &repo.repository_id, &branch.directory_id)?;
        let tree = next
            .directory_trees
            .get(&branch.head_tree_id)
            .ok_or_else(stale)?;
        if branch.materialized {
            if next.directory_selections.get(&branch.directory_id)
                != Some(&branch.directory_branch_id)
                || next.directory_tree_selections.get(&branch.directory_id)
                    != Some(&branch.head_tree_id)
                || tree.directory_branch_id != branch.directory_branch_id
                || tree.entries != branch.entries
                || tree.path != branch.path
                || tree.commit_id != branch.commit_id
                || tree.sequence != branch.sequence
                || tree.file_contexts != branch.file_contexts
                || tree.file_branches != branch.file_branches
                || tree.inherited_files != branch.inherited_files
                || tree.directory_refs != branch.directory_refs
            {
                return Err(invalid(
                    "new materialized directory is not a selected current snapshot",
                ));
            }
            verify_owner_snapshot(next, repo, tree, owner)?;
            return Ok(None);
        }
        let parent = branch.parent_branch_id.as_ref().ok_or_else(stale)?;
        if !owner.writable
            || next.directory_selections.get(&branch.directory_id) != Some(parent)
            || next.directory_tree_selections.get(&branch.directory_id) != Some(&tree.tree_id)
            || !inherits(next, parent, &tree.directory_branch_id)
            || tree.path != path
            || tree.entries != descendants(&owner.entries, &name_policy::path_key(path))
            || branch.path != tree.path
            || branch.commit_id != tree.commit_id
            || branch.entries != tree.entries
            || branch.file_branches != tree.file_branches
            || branch.directory_refs != tree.directory_refs
            || branch.sequence != next.sequence
            || branch.inherited_files
                != tree
                    .file_branches
                    .keys()
                    .map(|file| (file.clone(), branch.directory_branch_id.clone()))
                    .collect()
        {
            return Err(invalid(
                "directory fork is not bound to its exact current owner composition",
            ));
        }
        let expected_contexts = inherited_contexts(tree, &branch.directory_branch_id);
        if branch.file_contexts != expected_contexts {
            return Err(invalid("directory fork changes a source path context"));
        }
        verify_owner_snapshot(next, repo, tree, owner)?;
        for (directory, reference) in &tree.directory_refs {
            let child = next
                .directory_trees
                .get(&reference.tree_id)
                .ok_or_else(stale)?;
            let child_branch = next
                .directory_branches
                .get(&reference.directory_branch_id)
                .ok_or_else(stale)?;
            if child_branch.directory_id != *directory
                || child_branch.repository_id != repo.repository_id
                || next.directory_selections.get(directory) != Some(&reference.directory_branch_id)
                || next.directory_tree_selections.get(directory) != Some(&reference.tree_id)
                || !inherits(
                    next,
                    &reference.directory_branch_id,
                    &child.directory_branch_id,
                )
            {
                return Err(invalid(
                    "directory fork descendant is not the current selected source",
                ));
            }
            verify_owner_snapshot(next, repo, child, owner)?;
            if child
                .directory_refs
                .iter()
                .any(|(id, reference)| tree.directory_refs.get(id) != Some(reference))
            {
                return Err(invalid(
                    "directory fork descendant graph differs from its current root",
                ));
            }
        }
        let mut source = owner.clone();
        source.entries = Arc::new(branch.entries.clone());
        Ok(Some(source))
    }
}

fn inherited_contexts(tree: &DirectoryTree, branch: &str) -> BTreeMap<String, FileContext> {
    tree.file_contexts
        .iter()
        .map(|(path, context)| {
            let mut context = context.clone();
            context.selection_generation = 0;
            context.inherited_from = Some(branch.into());
            (path.clone(), context)
        })
        .collect()
}

fn verify_owner_snapshot(
    state: &State,
    repo: &crate::Repository,
    tree: &DirectoryTree,
    owner: &CurrentScope,
) -> Result<()> {
    let branch = state
        .directory_branches
        .get(&tree.directory_branch_id)
        .ok_or_else(stale)?;
    let key = name_policy::path_key(&tree.path);
    let identity_matches = if tree.path.is_empty() {
        branch.directory_id == repo.repository_id
    } else {
        at_key(&owner.entries, &key).is_some_and(|entry| {
            entry.kind == NamespaceEntryKind::Directory
                && entry.entry_id == branch.directory_id
                && entry.path() == tree.path
        })
    };
    if branch.repository_id != repo.repository_id
        || !identity_matches
        || !state
            .commits
            .get(&tree.commit_id)
            .is_some_and(|commit| commit.history_tree_id == repo.history_tree_id)
        || tree.entries != descendants(&owner.entries, &key)
    {
        return Err(invalid(
            "directory fork tree differs from its current owner source",
        ));
    }
    let files = tree
        .entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .collect::<Vec<_>>();
    let file_ids = files
        .iter()
        .map(|entry| entry.file_id.as_str())
        .collect::<BTreeSet<_>>();
    if tree.file_contexts.len() != files.len()
        || tree.file_branches.len() != file_ids.len()
        || tree
            .inherited_files
            .keys()
            .any(|id| !file_ids.contains(id.as_str()))
    {
        return Err(invalid(
            "directory fork tree has references outside its projected entries",
        ));
    }
    for entry in files {
        let context = tree
            .file_contexts
            .get(entry.lookup_key())
            .ok_or_else(stale)?;
        if state
            .file_contexts
            .get(&crate::business::context_key_from_key(
                &repo.repository_id,
                entry.lookup_key(),
            ))
            != Some(context)
            || !tree
                .file_branches
                .get(&entry.file_id)
                .and_then(|id| state.branches.get(id))
                .is_some_and(|branch| {
                    branch.file_id.as_deref() == Some(entry.file_id.as_str())
                        && branch.history_tree_id == repo.history_tree_id
                        && branch.state == BranchState::Active
                })
        {
            return Err(invalid(
                "directory fork tree does not carry the current path context",
            ));
        }
        validate_context(state, &repo.repository_id, entry, context)?;
    }
    let directories = tree
        .entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
        .map(|entry| entry.entry_id.as_str())
        .collect::<BTreeSet<_>>();
    for (id, reference) in &tree.directory_refs {
        if !directories.contains(id.as_str())
            || state.directory_selections.get(id) != Some(&reference.directory_branch_id)
            || state.directory_tree_selections.get(id) != Some(&reference.tree_id)
        {
            return Err(invalid(
                "directory fork tree contains a stale descendant reference",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/current_directory_branch_proof_tests.rs"]
mod tests;
