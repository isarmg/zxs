//! Publish immutable file nodes into only the directory references they target.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

/// Advance a public branch's metadata head while keeping its body immutable.
/// Tags preserve an unwritten branch's inheritance; adoption during a merge
/// completes that branch even though it needs no duplicate FULL.
pub(crate) fn advance_branch_node(
    state: &mut State,
    branch: &str,
    node: &str,
    materialize: bool,
    principal: &str,
    operation: &str,
) -> Result<()> {
    let previous = state.branches.get(branch).cloned().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::FileBranchNotFound,
            "file branch missing",
        )
    })?;
    let file = previous.file_id.as_deref().ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::FileBranchRequired,
            "an independent file branch is required",
        )
    })?;
    let version = super::file_nodes::node_version(state, file, node)?;
    let body = &state.versions[&version];
    let logical_size = body.logical_size;
    let current = state.commits.get(&previous.head_commit_id).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "file branch snapshot missing",
        )
    })?;
    let mut entries = current.entries.clone();
    let mut found = false;
    for entry in entries
        .iter_mut()
        .filter(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == file)
    {
        entry.version_id = version.clone();
        entry.size = logical_size;
        found = true;
    }
    if !found {
        return Err(Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "file branch body missing",
        ));
    }
    let pending = if materialize {
        BTreeMap::new()
    } else {
        previous.pending_file_versions.clone()
    };
    let id = new_id("commit");
    state.commits.insert(
        id.clone(),
        Commit {
            namespace_deletions: None,
            commit_id: id.clone(),
            history_tree_id: previous.history_tree_id,
            kind: "file_metadata".into(),
            parent_commit_ids: vec![current.commit_id.clone()],
            tree_id: new_id("tree"),
            sequence: state.sequence,
            operation_id: operation.into(),
            author: principal.into(),
            message: "Advance file metadata node".into(),
            entries,
            pending_file_versions: pending.clone(),
            provenance: Some(node.into()),
        },
    );
    let target = state.branches.get_mut(branch).expect("checked file branch");
    target.head_commit_id = id;
    target.file_node_id = Some(node.into());
    target.pending_file_versions = pending;
    target.generation = target.generation.checked_add(1).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::ResourceLimit,
            "branch generation exhausted",
        )
    })?;
    Ok(())
}

fn matches(
    context: &FileContext,
    entry: &Entry,
    branches: &BTreeSet<String>,
    node: Option<&str>,
) -> bool {
    branches.contains(&context.branch_id)
        && node.is_none_or(|node| context.node_id.as_deref().unwrap_or(&entry.version_id) == node)
}

fn advance(context: &mut FileContext, branch: &str, node: &str, materialize: bool) -> Result<()> {
    context.branch_id = branch.into();
    context.node_id = Some(node.into());
    if materialize {
        context.inherited_from = None;
    }
    context.selection_generation =
        context.selection_generation.checked_add(1).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "file context generation exhausted",
            )
        })?;
    Ok(())
}

/// A rewrite is keyed by the original immutable subtree, rather than by its
/// mutable named branch. A parent may freeze an older child composition.
struct Publication<'a> {
    file: &'a str,
    branches: &'a BTreeSet<String>,
    source: Option<&'a str>,
    target_branch: &'a str,
    target_node: &'a str,
    version: &'a str,
    size: u64,
    principal: &'a str,
    operation: &'a str,
    maximum: usize,
    rewrites: BTreeMap<String, String>,
    visiting: BTreeSet<String>,
}
impl Publication<'_> {
    fn rewrite(&mut self, state: &mut State, tree_id: &str) -> Result<String> {
        self.rewrite_projected(state, tree_id, None)
    }
    fn rewrite_projected(
        &mut self,
        state: &mut State,
        tree_id: &str,
        projection: Option<&DirectoryBranch>,
    ) -> Result<String> {
        let cache_key = projection
            .filter(|branch| !branch.materialized)
            .map(|branch| format!("{tree_id}:{}", branch.directory_branch_id))
            .unwrap_or_else(|| tree_id.into());
        if let Some(done) = self.rewrites.get(&cache_key) {
            return Ok(done.clone());
        }
        if !self.visiting.insert(cache_key.clone()) {
            return Err(Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "directory references contain a cycle",
            ));
        }
        let mut tree = state.directory_trees.get(tree_id).cloned().ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "frozen child subtree missing",
            )
        })?;
        if let Some(branch) = projection.filter(|branch| !branch.materialized) {
            // Pending scopes reuse an ancestor tree with their own inheritance
            // intent. Only a changed projection acquires its own subtree.
            tree.directory_branch_id = branch.directory_branch_id.clone();
            tree.file_contexts = branch.file_contexts.clone();
            tree.file_branches = branch.file_branches.clone();
            tree.inherited_files = branch.inherited_files.clone();
            tree.directory_refs = branch.directory_refs.clone();
        }
        let mut changed = false;
        for entry in tree
            .entries
            .iter_mut()
            .filter(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == self.file)
        {
            let mut context = tree
                .file_contexts
                .get(&entry.lookup_key)
                .cloned()
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::StateInvalid,
                        "frozen file path context missing",
                    )
                })?;
            if !matches(&context, entry, self.branches, self.source) {
                continue;
            }
            advance(
                &mut context,
                self.target_branch,
                self.target_node,
                self.source.is_none(),
            )?;
            entry.version_id = self.version.into();
            entry.size = self.size;
            tree.file_contexts.insert(entry.lookup_key.clone(), context);
            changed = true;
        }
        for reference in tree.directory_refs.values_mut() {
            let rewritten = self.rewrite(state, &reference.tree_id)?;
            if rewritten != reference.tree_id {
                reference.tree_id = rewritten;
                changed = true;
            }
        }
        if changed {
            // Compatibility caches never override the exact path contexts.
            if let Some(entry) = tree
                .entries
                .iter()
                .find(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == self.file)
                && let Some(context) = tree.file_contexts.get(&entry.lookup_key)
            {
                tree.file_branches
                    .insert(self.file.into(), context.branch_id.clone());
                if let Some(origin) = &context.inherited_from {
                    tree.inherited_files
                        .insert(self.file.into(), origin.clone());
                } else {
                    tree.inherited_files.remove(self.file);
                }
            }
            let previous = state.commits.get(&tree.commit_id).ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::StateInvalid,
                    "frozen directory commit missing",
                )
            })?;
            let mut full = tree.entries.clone();
            let key = name_policy::path_key(&tree.path);
            full.extend(
                previous
                    .entries
                    .iter()
                    .filter(|entry| {
                        entry.kind == NamespaceEntryKind::Directory
                            && (entry.lookup_key == key
                                || namespace::is_descendant(&key, &entry.lookup_key))
                    })
                    .cloned(),
            );
            validate_entries(&mut full, self.maximum)?;
            let id = new_id("commit");
            let pending = full
                .iter()
                .filter(|entry| entry.kind == NamespaceEntryKind::File)
                .filter_map(|entry| {
                    let context = tree.file_contexts.get(&entry.lookup_key)?;
                    (context.inherited_from.is_some()
                        || state
                            .branches
                            .get(&context.branch_id)
                            .is_some_and(|branch| {
                                branch.pending_file_versions.contains_key(&entry.file_id)
                            }))
                    .then(|| (entry.file_id.clone(), entry.version_id.clone()))
                })
                .collect();
            state.commits.insert(
                id.clone(),
                Commit {
                    namespace_deletions: None,
                    commit_id: id.clone(),
                    history_tree_id: previous.history_tree_id.clone(),
                    kind: "file_node_publication".into(),
                    parent_commit_ids: vec![previous.commit_id.clone()],
                    tree_id: new_id("tree"),
                    sequence: state.sequence,
                    operation_id: self.operation.into(),
                    author: self.principal.into(),
                    message: "Publish exact file references".into(),
                    entries: full,
                    pending_file_versions: pending,
                    provenance: None,
                },
            );
            tree.tree_id = new_id("dtree");
            tree.parent_tree_id = Some(tree_id.into());
            tree.commit_id = id;
            tree.sequence = state.sequence;
            state
                .directory_trees
                .insert(tree.tree_id.clone(), tree.clone());
        }
        self.visiting.remove(&cache_key);
        self.rewrites.insert(cache_key, tree.tree_id.clone());
        Ok(tree.tree_id)
    }
}

/// Publish only matching exact references. Historical child compositions are
/// rewritten from their own frozen tree, never replaced by a child's new HEAD.
/// No payload, version proof, namespace path or physical owner is changed.
pub(crate) struct NodePublication<'a> {
    pub(crate) file: &'a str,
    pub(crate) matching_branches: &'a BTreeSet<String>,
    pub(crate) matching_node: Option<&'a str>,
    pub(crate) target_branch: &'a str,
    pub(crate) target_node: &'a str,
    pub(crate) principal: &'a str,
    pub(crate) operation: &'a str,
    pub(crate) maximum: usize,
}

pub(crate) fn publish_node(state: &mut State, input: NodePublication<'_>) -> Result<Vec<String>> {
    let NodePublication {
        file,
        matching_branches,
        matching_node,
        target_branch,
        target_node,
        principal,
        operation,
        maximum,
    } = input;
    let version = super::file_nodes::node_version(state, file, target_node)?;
    let body = &state.versions[&version];
    let logical_size = body.logical_size;
    let repo = state
        .repositories
        .values()
        .find(|repo| repo.history_tree_id == body.history_tree_id)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::RepositoryNotFound,
                "file repository missing",
            )
        })?;
    let originals = state
        .directory_branches
        .values()
        .filter(|branch| branch.repository_id == repo.repository_id)
        .cloned()
        .collect::<Vec<_>>();
    let mut publication = Publication {
        file,
        branches: matching_branches,
        source: matching_node,
        target_branch,
        target_node,
        version: &version,
        size: logical_size,
        principal,
        operation,
        maximum,
        rewrites: BTreeMap::new(),
        visiting: BTreeSet::new(),
    };
    let mut affected = Vec::new();
    for original in &originals {
        let rewritten =
            publication.rewrite_projected(state, &original.head_tree_id, Some(original))?;
        if rewritten == original.head_tree_id {
            continue;
        }
        let tree = &state.directory_trees[&rewritten];
        let mut updated = original.clone();
        updated.head_tree_id = rewritten;
        updated.commit_id = tree.commit_id.clone();
        updated.entries = tree.entries.clone();
        updated.file_contexts = tree.file_contexts.clone();
        updated.file_branches = tree.file_branches.clone();
        updated.inherited_files = tree.inherited_files.clone();
        updated.directory_refs = tree.directory_refs.clone();
        updated.materialized = true;
        updated.sequence = state.sequence;
        state
            .directory_branches
            .insert(updated.directory_branch_id.clone(), updated);
        affected.push(original.directory_branch_id.clone());
    }
    // A currently selected child may itself be a frozen, non-head tree.
    // Advance only that exact composition and keep its mutable named HEAD apart.
    let selections = state.directory_tree_selections.clone();
    for (directory, tree) in selections {
        let Some(selected) = state
            .directory_selections
            .get(&directory)
            .and_then(|id| state.directory_branches.get(id))
        else {
            continue;
        };
        if selected.repository_id != repo.repository_id {
            continue;
        }
        let projection = originals.iter().find(|branch| {
            branch.directory_branch_id == selected.directory_branch_id
                && branch.head_tree_id == tree
        });
        let rewritten = publication.rewrite_projected(state, &tree, projection)?;
        if rewritten != tree {
            state.directory_tree_selections.insert(directory, rewritten);
        }
    }
    let main = state
        .branches
        .get(&repo.main_branch_id)
        .cloned()
        .ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "repository main branch missing",
            )
        })?;
    let previous = state.commits.get(&main.head_commit_id).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::StateInvalid,
            "repository head missing",
        )
    })?;
    let mut entries = previous.entries.clone();
    let mut changed_paths = Vec::new();
    for entry in entries
        .iter_mut()
        .filter(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == file)
    {
        let Some(mut context) = directory::context_at(state, &repo.repository_id, &entry.path)
        else {
            continue;
        };
        if !matches(&context, entry, matching_branches, matching_node) {
            continue;
        }
        advance(
            &mut context,
            target_branch,
            target_node,
            matching_node.is_none(),
        )?;
        entry.version_id = version.clone();
        entry.size = logical_size;
        state.file_contexts.insert(
            directory::context_key(&repo.repository_id, &entry.path),
            context,
        );
        changed_paths.push(entry.path.clone());
    }
    if !changed_paths.is_empty() {
        validate_entries(&mut entries, maximum)?;
        let id = new_id("commit");
        let pending = directory::snapshot_pending(state, &entries);
        state.commits.insert(
            id.clone(),
            Commit {
                namespace_deletions: None,
                commit_id: id.clone(),
                history_tree_id: repo.history_tree_id.clone(),
                kind: "file_node_publication".into(),
                parent_commit_ids: vec![previous.commit_id.clone()],
                tree_id: new_id("tree"),
                sequence: state.sequence,
                operation_id: operation.into(),
                author: principal.into(),
                message: "Publish current file references".into(),
                entries,
                pending_file_versions: pending,
                provenance: None,
            },
        );
        let main = state
            .branches
            .get_mut(&repo.main_branch_id)
            .expect("checked namespace");
        main.head_commit_id = id;
        main.generation = main.generation.checked_add(1).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "branch generation exhausted",
            )
        })?;
        if matching_node.is_none() {
            main.pending_file_versions.remove(file);
        }
        state
            .file_selections
            .insert(file.into(), target_branch.into());
        if matching_node.is_none() {
            state.file_write_origins.remove(file);
        }
        if originals.is_empty() {
            affected.extend(directory::record_directory_edits(
                state,
                &repo.repository_id,
                &changed_paths,
                None,
            )?);
        }
    }
    affected.sort();
    affected.dedup();
    Ok(affected)
}
