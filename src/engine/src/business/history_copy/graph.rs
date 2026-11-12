//! One complete file-graph projection used by single-file and aggregate moves.
//! It consumes an admitted occurrence, not a second lookup through an old HEAD.
use super::*;
mod commits;
pub(in crate::business) use commits::{CommitScope, project as project_commits};

#[derive(Clone, Copy)]
pub(in crate::business) struct IdentityScope<'a> {
    pub(in crate::business) operation: OperationId,
    /// None preserves every original single-file copy/move identity.
    pub(in crate::business) member: Option<&'a str>,
}
impl IdentityScope<'_> {
    pub(in crate::business) fn identity(self, source: &str) -> Result<String> {
        identity(self.operation, source)
    }
    pub(in crate::business) fn generated(self, prefix: &str, purpose: &str) -> Result<String> {
        let purpose = match self.member {
            Some(member) => format!("{prefix}_{member}:{purpose}"),
            None => format!("{prefix}_{purpose}"),
        };
        self.identity(&purpose)
    }
}

pub(in crate::business) struct CopiedGraph {
    pub(in crate::business) entry: Entry,
    pub(in crate::business) context: FileContext,
}

pub(in crate::business) struct GraphInput<'a> {
    pub source_repo: &'a Repository,
    pub target: &'a Repository,
    pub source: &'a CopySources,
    pub entry: &'a Entry,
    pub context: &'a FileContext,
    pub target_path: &'a name_policy::PortablePath,
    pub route: &'a CopyRoute,
    pub principal: &'a str,
    pub scope: IdentityScope<'a>,
    /// Aggregate moves project shared namespace/commit ancestry once.
    pub aggregate_ids: Option<&'a BTreeMap<String, String>>,
}

pub(in crate::business) fn project(
    previous: &State,
    next: &mut State,
    input: GraphInput<'_>,
) -> Result<CopiedGraph> {
    let GraphInput {
        source_repo,
        target,
        source,
        entry: old_entry,
        context,
        target_path,
        route,
        principal,
        scope,
        aggregate_ids,
    } = input;
    let operation = scope.operation;
    let commits = if aggregate_ids.is_none() {
        previous
            .commits
            .values()
            .filter(|commit| commit.history_tree_id == source_repo.history_tree_id)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let links = previous
        .merge_links
        .values()
        .filter(|link| {
            source.branches.contains(&link.source_branch_id)
                && source.branches.contains(&link.target_branch_id)
        })
        .collect::<Vec<_>>();
    let mut own_ids = BTreeMap::new();
    if aggregate_ids.is_none() {
        let mut add = |id: &str| -> Result<()> {
            own_ids.insert(id.to_owned(), scope.identity(id)?);
            Ok(())
        };
        add(&source.file)?;
        add(&old_entry.entry_id)?;
        for id in &source.versions {
            add(id)?;
        }
        for id in &source.branches {
            add(id)?;
        }
        for node in previous
            .file_path_nodes
            .values()
            .filter(|node| node.file_id == source.file)
        {
            add(&node.node_id)?;
        }
        for node in previous
            .file_metadata_nodes
            .values()
            .filter(|node| node.file_id == source.file)
        {
            add(&node.node_id)?;
        }
        for merge in previous
            .file_branch_merges
            .values()
            .filter(|merge| merge.file_id == source.file)
        {
            add(&merge.merge_id)?;
        }
        for link in &links {
            add(&link.merge_link_id)?;
        }
        for commit in &commits {
            add(&commit.commit_id)?;
            add(&commit.tree_id)?;
            for entry in &commit.entries {
                add(&entry.entry_id)?;
            }
        }
    }
    let ids = aggregate_ids.unwrap_or(&own_ids);
    let mapped = |id: &str| ids.get(id).cloned().ok_or_else(unsupported);
    let mapped_list = |items: &[String]| {
        items
            .iter()
            .map(|id| mapped(id))
            .collect::<Result<Vec<_>>>()
    };
    let file = mapped(&source.file)?;
    if source
        .versions
        .iter()
        .any(|id| ids.get(id).is_some_and(|id| next.versions.contains_key(id)))
        || next.file_names.contains_key(&file)
        || next
            .container_tags
            .contains_key(&file.parse::<storage_contract::FileId>()?)
    {
        return Err(invalid());
    }
    if route.slots.len() != target.target_copies as usize
        || route.slots.first() != Some(&target.primary_slot_id)
        || route.slots.iter().collect::<BTreeSet<_>>().len() != route.slots.len()
        || route.bindings.len() != route.slots.len()
    {
        return Err(invalid());
    }
    for id in &source.versions {
        let mut version = previous.versions[id].clone();
        version.version_id = mapped(id)?;
        version.file_id = file.clone();
        version.history_tree_id = target.history_tree_id.clone();
        version.creation_primary_slot_id = target.primary_slot_id.clone();
        version.source_version_ids = mapped_list(&version.source_version_ids)?;
        let placement = Placement {
            version_id: version.version_id.clone(),
            primary_slot_id: target.primary_slot_id.clone(),
            target_copies: target.target_copies,
            route_slot_ids: route.slots.clone(),
            topology_generation: next.topology_generation,
            binding_generations: route.bindings.clone(),
        };
        next.content_states
            .insert(version.version_id.clone(), ContentState::Available);
        next.placements
            .insert(version.version_id.clone(), placement);
        next.versions.insert(version.version_id.clone(), version);
    }
    if aggregate_ids.is_none() {
        project_commits(
            previous,
            next,
            target,
            commits,
            CommitScope {
                ids,
                files: [source.file.as_str()].into(),
                directories: BTreeSet::new(),
                trees: BTreeSet::new(),
                missing: unsupported,
            },
        )?;
    }
    for id in &source.branches {
        let mut branch = previous.branches[id].clone();
        branch.branch_id = mapped(id)?;
        branch.history_tree_id = target.history_tree_id.clone();
        branch.file_id = Some(file.clone());
        branch.head_commit_id = mapped(&branch.head_commit_id)?;
        branch.fork_commit_id = mapped(&branch.fork_commit_id)?;
        branch.file_node_id = branch.file_node_id.as_deref().map(mapped).transpose()?;
        branch.pending_file_versions = branch
            .pending_file_versions
            .get(&source.file)
            .map(|version| Ok::<_, Error>([(file.clone(), mapped(version)?)].into()))
            .transpose()?
            .unwrap_or_default();
        next.branches.insert(branch.branch_id.clone(), branch);
        if let Some(alias) = previous.file_branch_aliases.get(id) {
            next.file_branch_aliases.insert(mapped(id)?, mapped(alias)?);
        }
    }
    for original in previous
        .file_path_nodes
        .values()
        .filter(|node| node.file_id == source.file)
    {
        let mut node = original.clone();
        node.node_id = mapped(&node.node_id)?;
        node.file_id = file.clone();
        node.history_tree_id = target.history_tree_id.clone();
        node.version_id = mapped(&node.version_id)?;
        node.branch_id = mapped(&node.branch_id)?;
        node.parent_node_ids = mapped_list(&node.parent_node_ids)?;
        next.file_path_nodes.insert(node.node_id.clone(), node);
    }
    for original in previous
        .file_metadata_nodes
        .values()
        .filter(|node| node.file_id == source.file)
    {
        let mut node = original.clone();
        node.node_id = mapped(&node.node_id)?;
        node.file_id = file.clone();
        node.history_tree_id = target.history_tree_id.clone();
        node.version_id = mapped(&node.version_id)?;
        node.branch_id = mapped(&node.branch_id)?;
        node.parent_node_ids = mapped_list(&node.parent_node_ids)?;
        next.file_metadata_nodes.insert(node.node_id.clone(), node);
    }
    for original in previous
        .file_branch_merges
        .values()
        .filter(|merge| merge.file_id == source.file)
    {
        let mut merge = original.clone();
        merge.merge_id = mapped(&merge.merge_id)?;
        merge.file_id = file.clone();
        merge.history_tree_id = target.history_tree_id.clone();
        merge.input_branch_ids = mapped_list(&merge.input_branch_ids)?;
        merge.input_node_ids = mapped_list(&merge.input_node_ids)?;
        merge.chosen_node_id = mapped(&merge.chosen_node_id)?;
        merge.result_node_id = mapped(&merge.result_node_id)?;
        merge.surviving_branch_id = mapped(&merge.surviving_branch_id)?;
        merge.parent_node_ids = mapped_list(&merge.parent_node_ids)?;
        next.file_branch_merges
            .insert(merge.merge_id.clone(), merge);
    }
    for original in links {
        let mut link = original.clone();
        link.merge_link_id = mapped(&link.merge_link_id)?;
        link.history_tree_id = target.history_tree_id.clone();
        link.source_branch_id = mapped(&link.source_branch_id)?;
        link.target_branch_id = mapped(&link.target_branch_id)?;
        link.source_commit_id = mapped(&link.source_commit_id)?;
        link.target_commit_id = mapped(&link.target_commit_id)?;
        next.merge_links.insert(link.merge_link_id.clone(), link);
    }
    let node_ids = source
        .versions
        .iter()
        .chain(
            previous
                .file_path_nodes
                .values()
                .filter(|node| node.file_id == source.file)
                .map(|node| &node.node_id),
        )
        .chain(
            previous
                .file_metadata_nodes
                .values()
                .filter(|node| node.file_id == source.file)
                .map(|node| &node.node_id),
        );
    for id in node_ids {
        if let Some(parents) = previous.file_node_parents.get(id) {
            next.file_node_parents
                .insert(mapped(id)?, mapped_list(parents)?);
        }
        if let Some(note) = previous.file_path_notes.get(id) {
            next.file_path_notes.insert(mapped(id)?, note.clone());
        }
        if let Some(note) = previous.version_notes.get(id) {
            next.version_notes.insert(mapped(id)?, note.clone());
        }
    }
    let source_tags = previous
        .container_tags
        .get(&source.file.parse::<storage_contract::FileId>()?)
        .ok_or_else(crate::business::file_tags::invalid)?;
    next.container_tags.insert(
        file.parse::<storage_contract::FileId>()?,
        storage_contract::file_container_tags::ContainerTags::new(
            source_tags.tags().clone(),
            storage_contract::Generation::new(0),
        ),
    );
    if let Some(note) = previous.file_notes.get(&source.file) {
        next.file_notes.insert(file.clone(), note.clone());
    }
    if let Some(origin) = previous.file_write_origins.get(&source.file)
        && let Some(mapped_origin) = ids.get(origin).filter(|id| id.starts_with("dbr_"))
    {
        next.file_write_origins
            .insert(file.clone(), mapped_origin.clone());
    }
    if let Some(generation) = previous.file_selection_generations.get(&source.file) {
        next.file_selection_generations
            .insert(file.clone(), *generation);
    }
    let selected = mapped(&context.branch_id)?;
    if next
        .branches
        .get(&selected)
        .is_some_and(|branch| branch.file_id.as_deref() == Some(file.as_str()))
    {
        next.file_selections.insert(file.clone(), selected.clone());
    }
    next.file_names.insert(
        file.clone(),
        target_path
            .as_str()
            .rsplit('/')
            .next()
            .expect("invariant: target path is nonempty")
            .into(),
    );
    next.file_locations
        .insert(file.clone(), target_path.as_str().to_owned());
    let mut entry = old_entry.clone();
    entry.entry_id = mapped(&entry.entry_id)?;
    entry.file_id = file.clone();
    entry.version_id = mapped(&entry.version_id)?;
    entry.relocate(target_path.clone());
    let mut copied_context = context.clone();
    copied_context.branch_id = selected.clone();
    copied_context.node_id = context.node_id.as_deref().map(mapped).transpose()?;
    copied_context.inherited_from = context
        .inherited_from
        .as_ref()
        .and_then(|id| ids.get(id))
        .filter(|id| id.starts_with("dbr_"))
        .cloned();
    // The current occurrence has a new destination path. Historic paths remain
    // immutable, with an explicit new path node continuing the copied head.
    if aggregate_ids.is_none() && old_entry.path() != target_path.as_str() {
        let node_id = scope.generated("fnode", "arrival")?;
        let parent = copied_context
            .node_id
            .clone()
            .unwrap_or_else(|| entry.version_id.clone());
        next.file_path_nodes.insert(
            node_id.clone(),
            FilePathNode {
                node_id: node_id.clone(),
                history_tree_id: target.history_tree_id.clone(),
                file_id: file.clone(),
                version_id: entry.version_id.clone(),
                parent_node_ids: vec![parent],
                branch_id: selected.clone(),
                path: target_path.as_str().to_owned(),
                previous_path: old_entry.path().into(),
                operation_id: operation,
                author: principal.into(),
                sequence: next.sequence,
                created_at: route.created_at,
            },
        );
        copied_context.node_id = Some(node_id);
    }
    if aggregate_ids.is_none() {
        next.file_contexts.insert(
            directory::context_key_from_key(&target.repository_id, entry.lookup_key()),
            copied_context.clone(),
        );
    }
    Ok(CopiedGraph {
        entry,
        context: copied_context,
    })
}
