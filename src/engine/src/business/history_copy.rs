//! Online, source-retaining file-history fork. No source object is rewritten.
//! The same closed transformation is replayed before current-registry body I/O.
use super::*;
use data_serialization::{Deserialize, Serialize};
use json_codec::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const KIND: &str = "file.history.copy";

/// First online slice: one saved file into an empty, uninitialized repository
/// on another primary. Both exact namespace heads are part of the request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct FileHistoryCopyRequest {
    pub source_repository_id: String,
    pub source_path: String,
    pub expected_source_head: String,
    pub target_repository_id: String,
    pub target_name: String,
    pub expected_target_head: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct CopyRoute {
    #[serialization(with = "storage_contract::u64_string")]
    pub created_at: u64,
    pub slots: Vec<String>,
    #[serialization(with = "storage_contract::u64_string_map")]
    pub bindings: BTreeMap<String, u64>,
}

pub(crate) fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "file history copy differs from its complete source closure or exact request",
    )
}
fn unsupported() -> Error {
    Error::new(
        ErrorCode::UnsupportedOperation,
        "history copy requires an available, uniformly placed single-file history, unconfigured source and target placement/backup policies, and an empty uninitialized repository on another primary",
    )
}

/// Operation-bound identities make recovery reconstruct the complete write set,
/// instead of trusting an arbitrary old-to-new map stored in a receipt.
fn identity(operation: OperationId, source: &str) -> Result<String> {
    let (prefix, _) = source.split_once('_').ok_or_else(unsupported)?;
    let hash = object_hash("file-history-copy-identity", &json!([operation, source]))?;
    Ok(format!("{prefix}_{}", &hash[..32]))
}
fn generated(operation: OperationId, prefix: &str, purpose: &str) -> Result<String> {
    identity(operation, &format!("{prefix}_{purpose}"))
}

pub(crate) struct CopySources {
    pub file: String,
    pub versions: Vec<String>,
    pub branches: Vec<String>,
}

pub(crate) fn sources(
    previous: &State,
    view: Option<&crate::engine::CurrentNamespaces>,
    request: &FileHistoryCopyRequest,
) -> Result<CopySources> {
    let source_path = name_policy::PortablePath::parse(&request.source_path)?;
    name_policy::validate_name(&request.target_name)?;
    let source = previous
        .repositories
        .get(&request.source_repository_id)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::RepositoryNotFound,
                "history copy source repository missing",
            )
        })?;
    let target = previous
        .repositories
        .get(&request.target_repository_id)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::RepositoryNotFound,
                "history copy destination repository missing",
            )
        })?;
    let source_branch = &previous.branches[&source.main_branch_id];
    let target_branch = &previous.branches[&target.main_branch_id];
    if source_branch.head_commit_id != request.expected_source_head
        || target_branch.head_commit_id != request.expected_target_head
    {
        return Err(Error::new(
            ErrorCode::WorkspaceChanged,
            "history copy namespace head changed",
        ));
    }
    if source.primary_slot_id == target.primary_slot_id
        || source.history_tree_id == target.history_tree_id
        || source_branch.state != BranchState::Active
        || target_branch.state != BranchState::Active
        || previous
            .directory_branches
            .values()
            .any(|branch| branch.repository_id == target.repository_id)
        || previous
            .directory_git_repositories
            .contains_key(&target.repository_id)
        || previous.protection_policies.values().any(|policy| {
            policy.repository_id == source.repository_id
                || policy.repository_id == target.repository_id
        })
        || previous.placement_rules.values().any(|rule| {
            rule.repository_id == source.repository_id || rule.repository_id == target.repository_id
        })
        || previous
            .history_boundaries
            .values()
            .any(|boundary| boundary.history_tree_id == source.history_tree_id)
        || previous.histories[&source.history_tree_id]
            .provenance
            .is_some()
    {
        return Err(unsupported());
    }
    let source_entries = if let Some(view) = view {
        view.require_branch_write(previous, &source.main_branch_id)?;
        view.require_branch_write(previous, &target.main_branch_id)?;
        if !view
            .branch_entries(&target.main_branch_id, &target_branch.head_commit_id)?
            .is_empty()
        {
            return Err(unsupported());
        }
        view.branch_entries(&source.main_branch_id, &source_branch.head_commit_id)?
    } else {
        &previous.commits[&source_branch.head_commit_id].entries
    };
    // A masked old destination is not an empty root, nor is a parked target.
    if !previous.commits[&target_branch.head_commit_id]
        .entries
        .is_empty()
        || previous.branches.values().any(|branch| {
            branch.history_tree_id == target.history_tree_id
                && branch.branch_id != target.main_branch_id
        })
    {
        return Err(unsupported());
    }
    let entry = namespace::find_unordered_key(source_entries, source_path.key())
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .ok_or_else(|| Error::new(ErrorCode::FileNotFound, "history copy source file missing"))?;
    if previous.file_deletions.contains_key(&entry.file_id)
        || previous.file_backup_policies.contains_key(&entry.file_id)
        || previous.file_backup_paths.contains_key(&entry.file_id)
    {
        return Err(unsupported());
    }
    // A fork is not the background migration protocol. Do not silently choose
    // one historical route, discard a bound policy, or consolidate split bodies.
    let placement = &previous.placements[&entry.version_id];
    let versions = previous
        .versions
        .values()
        .filter(|version| version.file_id == entry.file_id)
        .map(|version| {
            let retained = &previous.placements[&version.version_id];
            if version.history_tree_id != source.history_tree_id
                || retained.primary_slot_id != source.primary_slot_id
                || retained.primary_slot_id != placement.primary_slot_id
                || retained.target_copies != placement.target_copies
                || retained.route_slot_ids != placement.route_slot_ids
                || retained.binding_generations != placement.binding_generations
                || previous
                    .content_states
                    .get(&version.version_id)
                    .is_some_and(|state| *state != ContentState::Available)
            {
                return Err(unsupported());
            }
            if let Some(view) = view {
                view.require_backup_configuration_source(previous, version)?;
            }
            Ok(version.version_id.clone())
        })
        .collect::<Result<Vec<_>>>()?;
    let branches = previous
        .branches
        .values()
        .filter(|branch| branch.file_id.as_deref() == Some(entry.file_id.as_str()))
        .map(|branch| branch.branch_id.clone())
        .collect::<Vec<_>>();
    let context = previous
        .file_contexts
        .get(&directory::context_key_from_key(
            &source.repository_id,
            entry.lookup_key(),
        ))
        .ok_or_else(unsupported)?;
    if !branches.contains(&context.branch_id)
        || context.inherited_from.is_some()
        || source_branch
            .pending_file_versions
            .contains_key(&entry.file_id)
        || !previous.branches[&context.branch_id]
            .pending_file_versions
            .is_empty()
        || file_nodes::branch_head(previous, &context.branch_id)?
            != *context.node_id.as_ref().unwrap_or(&entry.version_id)
        || previous
            .file_branch_aliases
            .contains_key(&context.branch_id)
    {
        return Err(unsupported());
    }
    Ok(CopySources {
        file: entry.file_id.clone(),
        versions,
        branches,
    })
}

/// Closed metadata projection. Original branch names, timestamps, version
/// sequences, tags, ancestry, merge facts and notes are retained under new IDs.
/// Directory history is projected only to this file, never imported as authority.
pub(crate) fn apply(
    previous: &State,
    next: &mut State,
    view: Option<&crate::engine::CurrentNamespaces>,
    request: &FileHistoryCopyRequest,
    route: &CopyRoute,
    principal: &str,
    operation: OperationId,
) -> Result<Value> {
    let source = sources(previous, view, request)?;
    let source_repo = &previous.repositories[&request.source_repository_id];
    let target = &previous.repositories[&request.target_repository_id];
    let old_entry = namespace::find_unordered(
        &previous.commits[&request.expected_source_head].entries,
        &request.source_path,
    )
    .ok_or_else(unsupported)?;
    let context = &previous.file_contexts
        [&directory::context_key_from_key(&source_repo.repository_id, old_entry.lookup_key())];
    let commits = previous
        .commits
        .values()
        .filter(|commit| commit.history_tree_id == source_repo.history_tree_id)
        .collect::<Vec<_>>();
    let links = previous
        .merge_links
        .values()
        .filter(|link| {
            source.branches.contains(&link.source_branch_id)
                && source.branches.contains(&link.target_branch_id)
        })
        .collect::<Vec<_>>();
    let mut ids = BTreeMap::new();
    {
        let mut add = |id: &str| -> Result<()> {
            ids.insert(id.to_owned(), identity(operation, id)?);
            Ok(())
        };
        add(&source.file)?;
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
    let mapped = |id: &str| ids.get(id).cloned().ok_or_else(unsupported);
    let mapped_list = |items: &[String]| {
        items
            .iter()
            .map(|id| mapped(id))
            .collect::<Result<Vec<_>>>()
    };
    let file = mapped(&source.file)?;
    if next.versions.contains_key(&mapped(&source.versions[0])?)
        || next.file_names.contains_key(&file)
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
    for original in commits {
        let mut commit = original.clone();
        commit.commit_id = mapped(&original.commit_id)?;
        commit.tree_id = mapped(&original.tree_id)?;
        commit.history_tree_id = target.history_tree_id.clone();
        commit.parent_commit_ids = mapped_list(&original.parent_commit_ids)?;
        if original.parent_commit_ids.is_empty() {
            commit.parent_commit_ids = vec![
                previous.histories[&target.history_tree_id]
                    .root_commit_id
                    .clone(),
            ];
            commit.kind = "file_history_projection".into();
        } else if original.kind == "directory_commit" {
            // Preserve the file snapshot, not a foreign directory Git marker.
            commit.kind = "file_history_projection".into();
        }
        // Provenance belonging to another file is not part of this projection.
        // Real file-node/commit/version references are remapped, never borrowed.
        commit.provenance = original
            .provenance
            .as_ref()
            .and_then(|id| ids.get(id).cloned());
        commit.entries = original
            .entries
            .iter()
            .filter(|entry| {
                entry.kind == NamespaceEntryKind::File && entry.file_id == source.file
                    || entry.kind == NamespaceEntryKind::Directory
                        && original.entries.iter().any(|file| {
                            file.file_id == source.file
                                && namespace::is_descendant(file.lookup_key(), entry.lookup_key())
                        })
            })
            .map(|entry| {
                let mut entry = entry.clone();
                entry.entry_id = mapped(&entry.entry_id)?;
                if entry.kind == NamespaceEntryKind::File {
                    entry.file_id = file.clone();
                    entry.version_id = mapped(&entry.version_id)?;
                }
                Ok(entry)
            })
            .collect::<Result<Vec<_>>>()?;
        commit.pending_file_versions = original
            .pending_file_versions
            .get(&source.file)
            .map(|version| Ok::<_, Error>([(file.clone(), mapped(version)?)].into()))
            .transpose()?
            .unwrap_or_default();
        // A namespace removal has paths, not file IDs. Preserve only removals
        // that covered this file in the immediately preceding snapshots.
        if let Some(deleted) = &mut commit.namespace_deletions {
            deleted.paths.retain(|deleted| {
                original.parent_commit_ids.iter().any(|parent| {
                    previous.commits[parent].entries.iter().any(|entry| {
                        entry.file_id == source.file
                            && (entry.path() == deleted.path
                                || namespace::is_descendant(
                                    entry.lookup_key(),
                                    &name_policy::path_key(&deleted.path),
                                ))
                    })
                })
            });
            if deleted.paths.is_empty() {
                commit.namespace_deletions = None;
            }
        }
        next.commits.insert(commit.commit_id.clone(), commit);
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
    if let Some(note) = previous.file_notes.get(&source.file) {
        next.file_notes.insert(file.clone(), note.clone());
    }
    if let Some(origin) = previous.file_write_origins.get(&source.file) {
        next.file_write_origins
            .insert(file.clone(), mapped(origin)?);
    }
    if let Some(generation) = previous.file_selection_generations.get(&source.file) {
        next.file_selection_generations
            .insert(file.clone(), *generation);
    }
    let selected = mapped(&context.branch_id)?;
    next.file_selections.insert(file.clone(), selected.clone());
    next.file_names
        .insert(file.clone(), request.target_name.clone());
    next.file_locations
        .insert(file.clone(), request.target_name.clone());
    let mut entry = old_entry.clone();
    entry.entry_id = mapped(&entry.entry_id)?;
    entry.file_id = file.clone();
    entry.version_id = mapped(&entry.version_id)?;
    entry.relocate(name_policy::PortablePath::parse(&request.target_name)?);
    let mut copied_context = context.clone();
    copied_context.branch_id = selected.clone();
    copied_context.node_id = context.node_id.as_deref().map(mapped).transpose()?;
    // The current occurrence has a new destination path. Historic paths remain
    // immutable, with an explicit new path node continuing the copied head.
    if old_entry.path() != request.target_name {
        let node_id = generated(operation, "fnode", "arrival")?;
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
                path: request.target_name.clone(),
                previous_path: old_entry.path().into(),
                operation_id: operation,
                author: principal.into(),
                sequence: next.sequence,
                created_at: route.created_at,
            },
        );
        copied_context.node_id = Some(node_id);
    }
    let arrival_node = copied_context.node_id.clone();
    next.file_contexts.insert(
        directory::context_key_from_key(&target.repository_id, entry.lookup_key()),
        copied_context,
    );
    let file_commit = generated(operation, "commit", "arrival-file")?;
    let id = generated(operation, "commit", "arrival")?;
    let selected_head = next.branches[&selected].head_commit_id.clone();
    let pending = next.branches[&selected].pending_file_versions.clone();
    next.commits.insert(
        id.clone(),
        Commit {
            commit_id: id.clone(),
            history_tree_id: target.history_tree_id.clone(),
            kind: "file_history_copy".into(),
            parent_commit_ids: vec![request.expected_target_head.clone()],
            tree_id: generated(operation, "tree", "arrival")?,
            sequence: next.sequence,
            operation_id: operation,
            author: principal.into(),
            message: "Copy complete file history; retain source".into(),
            entries: vec![entry],
            namespace_deletions: None,
            pending_file_versions: pending,
            provenance: None,
        },
    );
    let mut file_snapshot = next.commits[&id].clone();
    file_snapshot.commit_id = file_commit.clone();
    file_snapshot.tree_id = generated(operation, "tree", "arrival-file")?;
    file_snapshot.parent_commit_ids = vec![selected_head];
    next.commits.insert(file_commit.clone(), file_snapshot);
    for branch_id in [&selected, &target.main_branch_id] {
        let branch = next
            .branches
            .get_mut(branch_id)
            .expect("invariant: both copy branches were inserted or resolved");
        branch.head_commit_id = id.clone();
        branch.generation = branch
            .generation
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "branch generation exhausted"))?;
        if branch.file_id.is_some() {
            branch.head_commit_id = file_commit.clone();
            branch.file_node_id = arrival_node.clone();
        }
    }
    Ok(
        json!({"history_copy_request":request,"history_copy_route":route,"source_file_id":source.file,"file_id":file,"version_ids":source.versions.iter().map(|id| mapped(id)).collect::<Result<Vec<_>>>()?,"branch_id":target.main_branch_id,"history_tree_id":target.history_tree_id,"commit_id":id,"source_retained":true}),
    )
}

impl Engine {
    /// Copies every retained body and file graph in one ordinary durable
    /// publication. This is not ordinary copy, and never deletes the source.
    pub fn copy_file_history(
        &mut self,
        request: &FileHistoryCopyRequest,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.mutate(
            KIND,
            principal,
            operation,
            json!(request),
            |engine, next, prepared| {
                let source = sources(
                    engine.state(),
                    engine.current_namespaces.as_deref(),
                    request,
                )?;
                let target = &engine.state.repositories[&request.target_repository_id];
                for history in [
                    &engine.state.repositories[&request.source_repository_id].history_tree_id,
                    &target.history_tree_id,
                ] {
                    batch::require_completed_file_saves(engine, engine.state(), history)?;
                }
                let slots = engine
                    .planning_context()
                    .route(&target.primary_slot_id, target.target_copies)?;
                let bindings = slots
                    .iter()
                    .map(|slot| Ok((slot.clone(), engine.disk(slot)?.binding_generation)))
                    .collect::<Result<_>>()?;
                let route = CopyRoute {
                    created_at: now()?,
                    slots,
                    bindings,
                };
                let result = apply(
                    engine.state(),
                    next,
                    engine.current_namespaces.as_deref(),
                    request,
                    &route,
                    principal,
                    operation,
                )?;
                crate::history_package::validate_history_copy(next, &target.history_tree_id)?;
                for old in source.versions {
                    let id = identity(operation, &old)?;
                    prepared.push(crate::engine::Prepared {
                        version: next.versions[&id].clone(),
                        placement: next.placements[&id].clone(),
                        source: engine.valid_backup_replica(&old)?,
                    });
                }
                Ok(result)
            },
        )
    }
}
