//! Aggregate graph projection: one ID map and one shared commit projection.
//! File records reuse the existing single-file transformer.
use super::*;
use history_copy::{
    CopyRoute, CopySources,
    graph::{self, GraphInput, IdentityScope},
};

pub(super) struct Graph {
    pub ids: BTreeMap<String, String>,
    pub versions: BTreeMap<String, String>,
    pub lifecycle: Vec<history_copy::retained::CopiedLifecycle>,
    commits: BTreeSet<String>,
}
impl Graph {
    pub fn id(&self, id: &str) -> Result<String> {
        self.ids.get(id).cloned().ok_or_else(invalid)
    }
    pub fn entry(&self, entry: &Entry) -> Result<Entry> {
        let mut mapped = entry.clone();
        mapped.entry_id = self.id(&entry.entry_id)?;
        if entry.kind == NamespaceEntryKind::File {
            mapped.file_id = self.id(&entry.file_id)?;
            mapped.version_id = self.id(&entry.version_id)?;
        }
        Ok(mapped)
    }
    pub fn context(&self, context: &FileContext) -> Result<FileContext> {
        Ok(FileContext {
            branch_id: self.id(&context.branch_id)?,
            node_id: context
                .node_id
                .as_deref()
                .map(|id| self.id(id))
                .transpose()?,
            inherited_from: context
                .inherited_from
                .as_ref()
                .and_then(|id| self.ids.get(id))
                .filter(|id| id.starts_with("dbr_"))
                .cloned(),
            selection_generation: context.selection_generation,
        })
    }
}

fn retained_commits(previous: &State, unit: &MoveUnit) -> BTreeSet<String> {
    let mut pending = BTreeSet::new();
    // An ordinary namespace can save files before any independent branch
    // exists. Their captured main still owns the original save facts.
    if unit
        .entries
        .iter()
        .filter_map(|member| member.context.as_ref())
        .any(|context| context.branch_id == unit.source_repository.main_branch_id)
    {
        pending.insert(
            previous.branches[&unit.source_repository.main_branch_id]
                .head_commit_id
                .clone(),
        );
    }
    for branch in unit.files.values().flat_map(|file| &file.branch_ids) {
        let branch = &previous.branches[branch];
        pending.insert(branch.head_commit_id.clone());
        pending.insert(branch.fork_commit_id.clone());
    }
    for directory in unit.directories.values() {
        pending.extend(
            directory
                .tree_ids
                .iter()
                .map(|id| previous.directory_trees[id].commit_id.clone()),
        );
        pending.extend(
            directory
                .repository
                .deletions
                .values()
                .map(|node| node.namespace_head.clone()),
        );
    }
    let selected_branches = unit
        .files
        .values()
        .flat_map(|file| file.branch_ids.iter())
        .collect::<BTreeSet<_>>();
    for link in previous.merge_links.values().filter(|link| {
        selected_branches.contains(&link.source_branch_id)
            && selected_branches.contains(&link.target_branch_id)
    }) {
        pending.insert(link.source_commit_id.clone());
        pending.insert(link.target_commit_id.clone());
    }
    let mut retained = BTreeSet::new();
    while let Some(id) = pending.pop_first() {
        if !retained.insert(id.clone()) {
            continue;
        }
        pending.extend(previous.commits[&id].parent_commit_ids.iter().cloned());
    }
    retained
}

fn identities(
    previous: &State,
    unit: &MoveUnit,
    target: &Repository,
    operation: OperationId,
) -> Result<Graph> {
    let mut selected = BTreeSet::new();
    let files = unit
        .files
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    for (file, members) in &unit.files {
        selected.insert(file.clone());
        selected.extend(members.version_ids.iter().cloned());
        selected.extend(members.branch_ids.iter().cloned());
    }
    for member in &unit.entries {
        selected.insert(member.source.entry_id.clone());
    }
    let commits = retained_commits(previous, unit);
    for id in &commits {
        let commit = &previous.commits[id];
        selected.insert(commit.commit_id.clone());
        selected.insert(commit.tree_id.clone());
        selected.extend(commit.entries.iter().map(|entry| entry.entry_id.clone()));
    }
    for node in previous
        .file_deletions
        .values()
        .filter(|node| files.contains(node.file_id.as_str()))
    {
        selected.insert(node.node_id.clone());
    }
    for node in previous
        .file_path_nodes
        .values()
        .filter(|node| files.contains(node.file_id.as_str()))
    {
        selected.insert(node.node_id.clone());
    }
    for node in previous
        .file_metadata_nodes
        .values()
        .filter(|node| files.contains(node.file_id.as_str()))
    {
        selected.insert(node.node_id.clone());
    }
    for merge in previous
        .file_branch_merges
        .values()
        .filter(|node| files.contains(node.file_id.as_str()))
    {
        selected.insert(merge.merge_id.clone());
    }
    for link in previous
        .merge_links
        .values()
        .filter(|link| {
            selected.contains(&link.source_branch_id) && selected.contains(&link.target_branch_id)
        })
        .map(|link| link.merge_link_id.clone())
        .collect::<Vec<_>>()
    {
        selected.insert(link);
    }
    for (id, directory) in &unit.directories {
        selected.insert(id.clone());
        selected.extend(directory.branch_ids.iter().cloned());
        selected.extend(directory.tree_ids.iter().cloned());
        for tree in &directory.tree_ids {
            selected.extend(
                previous.directory_trees[tree]
                    .entries
                    .iter()
                    .map(|entry| entry.entry_id.clone()),
            );
        }
        for deletion in directory.repository.deletions.values() {
            selected.insert(deletion.node_id.clone());
            selected.insert(deletion.directory_id.clone());
        }
    }
    let mut ids = selected
        .into_iter()
        .map(|id| Ok((id.clone(), history_copy::identity(operation, &id)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    ids.insert(
        unit.source_repository.main_branch_id.clone(),
        target.main_branch_id.clone(),
    );
    // A file may still borrow its current contents from an exterior directory
    // branch. That owner's branch is not copied; arrival creates its own file
    // branch from the same pending version under the target namespace.
    let contexts = unit
        .entries
        .iter()
        .filter_map(|member| member.context.as_ref())
        .chain(
            unit.directories
                .values()
                .flat_map(|directory| &directory.tree_ids)
                .flat_map(|tree| previous.directory_trees[tree].file_contexts.values()),
        );
    for context in contexts {
        if !ids.contains_key(&context.branch_id)
            && previous.directory_branches.contains_key(&context.branch_id)
        {
            ids.insert(context.branch_id.clone(), target.main_branch_id.clone());
        }
    }
    for node in previous
        .file_deletions
        .values()
        .filter(|node| unit.files.contains_key(&node.file_id))
    {
        ids.insert(
            node.node_id.clone(),
            IdentityScope {
                operation,
                member: None,
            }
            .generated("fdel", &format!("import:{}", ids[&node.file_id]))?,
        );
    }
    for directory in unit.directories.values() {
        for node in directory.repository.deletions.values() {
            let mapped_directory = ids[&node.directory_id].clone();
            ids.insert(
                node.node_id.clone(),
                crate::file_deletion::directory::identity(operation, "ddel", &mapped_directory)?,
            );
        }
    }
    let versions = unit
        .files
        .values()
        .flat_map(|file| file.version_ids.iter())
        .map(|id| (id.clone(), ids[id].clone()))
        .collect();
    Ok(Graph {
        ids,
        versions,
        lifecycle: Vec::new(),
        commits,
    })
}

fn commits(
    previous: &State,
    next: &mut State,
    unit: &MoveUnit,
    target: &Repository,
    graph: &Graph,
) -> Result<()> {
    let files = unit
        .files
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let directories = unit
        .entries
        .iter()
        .filter(|entry| entry.source.kind == NamespaceEntryKind::Directory)
        .map(|entry| entry.source.entry_id.as_str())
        .chain(unit.directories.keys().map(String::as_str))
        .chain(
            unit.directories
                .values()
                .flat_map(|directory| &directory.tree_ids)
                .flat_map(|tree| previous.directory_trees[tree].entries.iter())
                .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
                .map(|entry| entry.entry_id.as_str()),
        )
        .collect::<BTreeSet<_>>();
    for id in &graph.commits {
        let original = &previous.commits[id];
        let mut commit = original.clone();
        commit.commit_id = graph.id(&original.commit_id)?;
        commit.tree_id = graph.id(&original.tree_id)?;
        commit.history_tree_id = target.history_tree_id.clone();
        commit.parent_commit_ids = original
            .parent_commit_ids
            .iter()
            .map(|id| graph.id(id))
            .collect::<Result<_>>()?;
        let retained_directory_commit = original.kind == "directory_commit"
            && original.provenance.as_ref().is_some_and(|tree| {
                unit.directories
                    .values()
                    .any(|directory| directory.tree_ids.contains(tree))
            });
        if !retained_directory_commit {
            if commit.parent_commit_ids.is_empty() {
                commit.parent_commit_ids.push(
                    next.histories[&target.history_tree_id]
                        .root_commit_id
                        .clone(),
                );
                commit.kind = "file_history_projection".into();
            } else if original.kind == "directory_commit" {
                commit.kind = "file_history_projection".into();
            }
        }
        let selected = original
            .entries
            .iter()
            .filter(|entry| {
                (entry.kind == NamespaceEntryKind::File && files.contains(entry.file_id.as_str()))
                    || directories.contains(entry.entry_id.as_str())
            })
            .map(Entry::lookup_key)
            .collect::<BTreeSet<_>>();
        commit.entries = original
            .entries
            .iter()
            .filter(|entry| {
                selected.contains(entry.lookup_key())
                    || (entry.kind == NamespaceEntryKind::Directory
                        && selected
                            .iter()
                            .any(|path| namespace::is_descendant(path, entry.lookup_key())))
            })
            .map(|entry| graph.entry(entry))
            .collect::<Result<_>>()?;
        commit.pending_file_versions = original
            .pending_file_versions
            .iter()
            .filter(|(file, _)| files.contains(file.as_str()))
            .map(|(file, version)| Ok((graph.id(file)?, graph.id(version)?)))
            .collect::<Result<_>>()?;
        commit.provenance = original
            .provenance
            .as_ref()
            .and_then(|id| graph.ids.get(id).cloned());
        // Old removal paths remain historical. Keep only ones affecting a
        // selected member in the immediate parent snapshots.
        if let Some(deletions) = &mut commit.namespace_deletions {
            deletions.paths.retain(|deleted| {
                original.parent_commit_ids.iter().any(|parent| {
                    previous.commits[parent].entries.iter().any(|entry| {
                        (files.contains(entry.file_id.as_str())
                            || directories.contains(entry.entry_id.as_str()))
                            && (entry.path() == deleted.path
                                || namespace::is_descendant(
                                    entry.lookup_key(),
                                    &name_policy::path_key(&deleted.path),
                                ))
                    })
                })
            });
            if deletions.paths.is_empty() {
                commit.namespace_deletions = None;
            }
        }
        next.commits.insert(commit.commit_id.clone(), commit);
    }
    Ok(())
}

fn occurrence<'a>(
    previous: &'a State,
    unit: &'a MoveUnit,
    file: &str,
) -> Result<(Entry, FileContext, name_policy::PortablePath)> {
    if let Some(member) = unit
        .entries
        .iter()
        .find(|member| member.source.file_id == file)
    {
        return Ok((
            member.source.clone(),
            member
                .context
                .clone()
                .expect("invariant: current file member has context"),
            member.destination.clone(),
        ));
    }
    for directory in unit.directories.values() {
        for tree_id in &directory.tree_ids {
            let tree = &previous.directory_trees[tree_id];
            if let Some(entry) = tree.entries.iter().find(|entry| entry.file_id == file) {
                let context = tree
                    .file_contexts
                    .get(entry.lookup_key())
                    .cloned()
                    .unwrap_or_else(|| FileContext {
                        branch_id: tree.file_branches[file].clone(),
                        node_id: None,
                        inherited_from: tree.inherited_files.get(file).cloned(),
                        selection_generation: 0,
                    });
                return Ok((entry.clone(), context, entry.location().clone()));
            }
        }
    }
    // A frozen pending member can retain another file's version. Its retained
    // graph is copied as a hidden dependency, not added to today's namespace.
    for branch in previous
        .branches
        .values()
        .filter(|branch| branch.file_id.as_deref() == Some(file))
    {
        if let Some(entry) = previous.commits[&branch.head_commit_id]
            .entries
            .iter()
            .find(|entry| entry.file_id == file)
        {
            return Ok((
                entry.clone(),
                FileContext {
                    branch_id: branch.branch_id.clone(),
                    node_id: branch.file_node_id.clone(),
                    inherited_from: None,
                    selection_generation: 0,
                },
                entry.location().clone(),
            ));
        }
    }
    Err(invalid())
}

pub(super) fn copy(
    previous: &State,
    next: &mut State,
    unit: &MoveUnit,
    target: &Repository,
    route: &CopyRoute,
    principal: &str,
    operation: OperationId,
) -> Result<Graph> {
    let mut graph = identities(previous, unit, target, operation)?;
    commits(previous, next, unit, target, &graph)?;
    let mut source_projection = None;
    for (file, member) in &unit.files {
        let (entry, context, target_path) = occurrence(previous, unit, file)?;
        let source = CopySources {
            file: file.clone(),
            versions: member.version_ids.clone(),
            branches: member.branch_ids.clone(),
        };
        graph::project(
            previous,
            next,
            GraphInput {
                source_repo: &unit.source_repository,
                target,
                source: &source,
                entry: &entry,
                context: &context,
                target_path: &target_path,
                route,
                principal,
                scope: IdentityScope {
                    operation,
                    member: Some(file),
                },
                aggregate_ids: Some(&graph.ids),
            },
        )?;
        if !member.version_ids.is_empty() {
            let projection = source_projection
                .get_or_insert_with(|| crate::state_projection::ProjectionSnapshot::new(previous));
            graph.lifecycle.push(history_copy::retained::capture_mapped(
                projection.view(),
                file,
                &unit.source_repository.history_tree_id,
                &target.history_tree_id,
                operation,
            )?);
        }
    }
    Ok(graph)
}
