//! Explicit file-branch adoption with immutable bodies and durable provenance.
use super::*;
use json_codec::json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
pub(super) enum MergeResultPolicy<'a> {
    ReuseChosenInput,
    AlwaysNewNode {
        current_inputs: Option<&'a super::file_history_merges::CurrentHistoryMergeInputs>,
    },
}

fn reaches(state: &State, start: &str, needle: &str) -> bool {
    let mut queue = vec![start.to_owned()];
    let mut seen = BTreeSet::new();
    while let Some(node) = queue.pop() {
        if node == needle {
            return true;
        }
        if seen.insert(node.clone()) {
            queue.extend(super::file_nodes::node_parents(state, &node));
        }
    }
    false
}

fn advance_current_merge_branch(
    state: &mut State,
    input: &super::file_publication::NodePublication<'_>,
    mut entries: Vec<Entry>,
) -> Result<()> {
    let branch = state.branches[input.target_branch].clone();
    let version = super::file_nodes::node_version(state, input.file, input.target_node)?;
    let size = state.versions[&version].logical_size;
    for entry in entries
        .iter_mut()
        .filter(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == input.file)
    {
        entry.version_id = version.clone();
        entry.size = size;
    }
    let id = new_id("commit")?;
    state.commits.insert(
        id.clone(),
        Commit {
            namespace_deletions: None,
            commit_id: id.clone(),
            history_tree_id: branch.history_tree_id,
            kind: "file_metadata".into(),
            parent_commit_ids: vec![branch.head_commit_id],
            tree_id: new_id("tree")?,
            sequence: state.sequence,
            operation_id: input.operation,
            author: input.principal.into(),
            message: "Advance current file merge node".into(),
            entries,
            pending_file_versions: BTreeMap::new(),
            provenance: Some(input.target_node.into()),
        },
    );
    let target = state
        .branches
        .get_mut(input.target_branch)
        .expect("invariant: merge survivor was admitted");
    target.head_commit_id = id;
    target.file_node_id = Some(input.target_node.into());
    target.pending_file_versions.clear();
    target.generation = target.generation.checked_add(1).ok_or_else(|| {
        Error::new(
            storage_contract::ErrorCode::ResourceLimit,
            "branch generation exhausted",
        )
    })?;
    Ok(())
}

fn merge_context(
    context: &mut FileContext,
    input: &super::file_publication::NodePublication<'_>,
) -> Result<()> {
    context.branch_id = input.target_branch.into();
    context.node_id = Some(input.target_node.into());
    context.inherited_from = None;
    context.selection_generation =
        context.selection_generation.checked_add(1).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "file context generation exhausted",
            )
        })?;
    Ok(())
}

/// Only already-admitted current compositions enter this publisher. The source
/// verifier has rejected frozen/non-head child imports, so bottom-up rewrites
/// preserve exact references without traversing arbitrary immutable history.
fn publish_current_merge(
    engine: &Engine,
    state: &mut State,
    directories: Vec<DirectoryBranch>,
    input: super::file_publication::NodePublication<'_>,
) -> Result<Vec<String>> {
    let view = engine
        .admitted_current_namespaces()?
        .expect("invariant: projected merge uses an admitted current registry");
    let version = super::file_nodes::node_version(state, input.file, input.target_node)?;
    let body = &state.versions[&version];
    let merge = CurrentMerge {
        view,
        repository: engine
            .repository_for_history(&body.history_tree_id)?
            .clone(),
        size: body.logical_size,
        version,
        input,
    };
    let mut rewrites = BTreeMap::<String, String>::new();
    for branch in directories {
        if let Some((branch, tree)) =
            merge.rewrite_directory(engine.state(), state, branch, &rewrites)?
        {
            rewrites.insert(branch, tree);
        }
    }
    let mut affected = rewrites.into_keys().collect::<Vec<_>>();
    affected.extend(merge.rewrite_main(engine, state)?);
    affected.sort();
    affected.dedup();
    Ok(affected)
}

struct CurrentMerge<'a> {
    view: &'a crate::engine::CurrentNamespaces,
    repository: Repository,
    version: String,
    size: u64,
    input: super::file_publication::NodePublication<'a>,
}

impl CurrentMerge<'_> {
    fn rewrite_directory(
        &self,
        original: &State,
        state: &mut State,
        mut branch: DirectoryBranch,
        rewrites: &BTreeMap<String, String>,
    ) -> Result<Option<(String, String)>> {
        let input = &self.input;
        let mut changed = false;
        for entry in branch
            .entries
            .iter_mut()
            .filter(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == input.file)
        {
            let context = branch
                .file_contexts
                .get_mut(entry.lookup_key())
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::StateInvalid,
                        "current file path context missing",
                    )
                })?;
            if input.matching_branches.contains(&context.branch_id) {
                merge_context(context, input)?;
                entry.version_id = self.version.clone();
                entry.size = self.size;
                changed = true;
            }
        }
        for reference in branch.directory_refs.values_mut() {
            if let Some(tree) = rewrites.get(&reference.directory_branch_id) {
                reference.tree_id = tree.clone();
                changed = true;
            }
        }
        if !changed {
            return Ok(None);
        }
        self.view.require_file_merge_directory(original, &branch)?;
        // Compatibility maps never replace path-local ownership. Update them
        // from the resulting first occurrence, leaving other paths independent.
        if let Some(entry) = branch
            .entries
            .iter()
            .find(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == input.file)
        {
            let context = &branch.file_contexts[entry.lookup_key()];
            branch
                .file_branches
                .insert(input.file.into(), context.branch_id.clone());
            if let Some(origin) = &context.inherited_from {
                branch
                    .inherited_files
                    .insert(input.file.into(), origin.clone());
            } else {
                branch.inherited_files.remove(input.file);
            }
        }
        self.freeze_directory(state, branch).map(Some)
    }

    fn freeze_directory(
        &self,
        state: &mut State,
        mut branch: DirectoryBranch,
    ) -> Result<(String, String)> {
        let input = &self.input;
        // A selected projection can relocate an old tree. Its commit is still
        // the real ancestry source, while ancestor directory identities must
        // come from the admitted current owner at the projected path.
        let owner = &state.branches[&self.repository.main_branch_id];
        let current = self
            .view
            .branch_entries(&owner.branch_id, &owner.head_commit_id)?;
        let mut full = branch.entries.clone();
        let key = name_policy::path_key(&branch.path);
        full.extend(
            current
                .iter()
                .filter(|entry| {
                    entry.kind == NamespaceEntryKind::Directory
                        && (entry.lookup_key() == key
                            || namespace::is_descendant(&key, entry.lookup_key()))
                })
                .cloned(),
        );
        validate_entries(&mut full, input.maximum)?;
        let pending = branch
            .entries
            .iter()
            .filter_map(|entry| {
                let context = branch.file_contexts.get(entry.lookup_key())?;
                (context.inherited_from.is_some()
                    || state.branches[&context.branch_id]
                        .pending_file_versions
                        .contains_key(&entry.file_id))
                .then(|| (entry.file_id.clone(), entry.version_id.clone()))
            })
            .collect();
        let commit_id = new_id("commit")?;
        state.commits.insert(
            commit_id.clone(),
            Commit {
                namespace_deletions: None,
                commit_id: commit_id.clone(),
                history_tree_id: self.repository.history_tree_id.clone(),
                kind: "file_node_publication".into(),
                parent_commit_ids: vec![branch.commit_id.clone()],
                tree_id: new_id("tree")?,
                sequence: state.sequence,
                operation_id: input.operation,
                author: input.principal.into(),
                message: "Publish current directory file references".into(),
                entries: full,
                pending_file_versions: pending,
                provenance: None,
            },
        );
        let tree_id = new_id("dtree")?;
        state.directory_trees.insert(
            tree_id.clone(),
            DirectoryTree {
                tree_id: tree_id.clone(),
                directory_branch_id: branch.directory_branch_id.clone(),
                parent_tree_id: Some(branch.head_tree_id.clone()),
                commit_id: commit_id.clone(),
                path: branch.path.clone(),
                entries: branch.entries.clone(),
                file_contexts: branch.file_contexts.clone(),
                file_branches: branch.file_branches.clone(),
                inherited_files: branch.inherited_files.clone(),
                directory_refs: branch.directory_refs.clone(),
                sequence: state.sequence,
            },
        );
        if state.directory_selections.get(&branch.directory_id) == Some(&branch.directory_branch_id)
        {
            state
                .directory_tree_selections
                .insert(branch.directory_id.clone(), tree_id.clone());
        }
        branch.head_tree_id = tree_id.clone();
        branch.commit_id = commit_id;
        branch.materialized = true;
        branch.sequence = state.sequence;
        let id = branch.directory_branch_id.clone();
        state.directory_branches.insert(id.clone(), branch);
        Ok((id, tree_id))
    }

    fn rewrite_main(&self, engine: &Engine, state: &mut State) -> Result<Vec<String>> {
        let input = &self.input;
        let repository = &self.repository;
        let view = self.view;
        let main = &engine.state().branches[&repository.main_branch_id];
        let source = engine.namespace_snapshot(&repository.main_branch_id)?;
        let mut entries = source.entries().to_vec();
        let mut changed_paths = Vec::new();
        for entry in entries
            .iter_mut()
            .filter(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == input.file)
        {
            let Some(mut context) =
                directory::context_at_key(state, &repository.repository_id, entry.lookup_key())
            else {
                continue;
            };
            if !input.matching_branches.contains(&context.branch_id) {
                continue;
            }
            merge_context(&mut context, input)?;
            entry.version_id = self.version.clone();
            entry.size = self.size;
            state.file_contexts.insert(
                directory::context_key_from_key(&repository.repository_id, entry.lookup_key()),
                context,
            );
            changed_paths.push(entry.path().to_owned());
        }
        let mut affected = Vec::new();
        if !changed_paths.is_empty() {
            view.validate_draft(&repository.main_branch_id, &entries)?;
            view.remove_hidden_contexts(
                state,
                &repository.repository_id,
                &repository.main_branch_id,
            );
            let pending = directory::snapshot_pending(state, &entries);
            let live: BTreeSet<_> = entries
                .iter()
                .filter(|entry| entry.kind == NamespaceEntryKind::File)
                .map(|entry| entry.file_id.clone())
                .collect();
            let id = new_id("commit")?;
            state.commits.insert(
                id.clone(),
                Commit {
                    namespace_deletions: None,
                    commit_id: id.clone(),
                    history_tree_id: repository.history_tree_id.clone(),
                    kind: "file_node_publication".into(),
                    parent_commit_ids: vec![main.head_commit_id.clone()],
                    tree_id: new_id("tree")?,
                    sequence: state.sequence,
                    operation_id: input.operation,
                    author: input.principal.into(),
                    message: "Publish current file references".into(),
                    entries,
                    pending_file_versions: pending,
                    provenance: None,
                },
            );
            let main = state
                .branches
                .get_mut(&repository.main_branch_id)
                .expect("invariant: current merge owner was admitted");
            main.head_commit_id = id;
            main.pending_file_versions
                .retain(|file, _| live.contains(file) && file != input.file);
            main.generation = main.generation.checked_add(1).ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::ResourceLimit,
                    "branch generation exhausted",
                )
            })?;
            state
                .file_selections
                .insert(input.file.into(), input.target_branch.into());
            state.file_write_origins.remove(input.file);
            if !state
                .directory_branches
                .values()
                .any(|branch| branch.repository_id == repository.repository_id)
            {
                affected.extend(directory::record_projected_directory_edits(
                    state,
                    &repository.repository_id,
                    &changed_paths,
                    None,
                    Some(view),
                )?);
            }
        }
        affected.sort();
        affected.dedup();
        Ok(affected)
    }
}

impl Engine {
    pub fn merge_file_branches(
        &mut self,
        request: FileBranchMergeRequest<'_>,
    ) -> Result<OperationResult> {
        let body = request.body();
        self.mutate(
            "file.branches.merge",
            request.principal,
            request.operation_id,
            body,
            |engine, state, _| {
                merge_change(engine, state, request, MergeResultPolicy::ReuseChosenInput)
            },
        )
    }
}

pub(super) fn merge_change(
    engine: &Engine,
    state: &mut State,
    request: FileBranchMergeRequest<'_>,
    policy: MergeResultPolicy<'_>,
) -> Result<json_codec::Value> {
    let FileBranchMergeRequest {
        file_id: file,
        inputs,
        chosen_node_id: chosen_node,
        surviving_branch_id: survivor,
        location,
        principal,
        operation_id: operation,
    } = request;
    super::file_requests::validate_location(engine, file, location)?;
    if inputs.len() < 2 || inputs.len() > engine.config.max_entries {
        return Err(Error::new(
            storage_contract::ErrorCode::InvalidRequest,
            "merge requires at least two bounded file branches",
        ));
    }
    let version = super::file_nodes::node_version(state, file, chosen_node)?;
    let body = state.versions[&version].clone();
    let repository = engine.repository_for_history(&body.history_tree_id)?;
    let historical_inputs = match policy {
        MergeResultPolicy::AlwaysNewNode { current_inputs, .. } => current_inputs,
        MergeResultPolicy::ReuseChosenInput => None,
    };
    let branches = inputs
        .iter()
        .map(|input| input.branch_id.clone())
        .collect::<BTreeSet<_>>();
    if branches.len() != inputs.len() {
        return Err(Error::new(
            storage_contract::ErrorCode::InvalidRequest,
            "merge branch inputs must be distinct",
        ));
    }
    let projected = if let Some(view) = engine.admitted_current_namespaces()? {
        view.require_branch_write(state, &repository.main_branch_id)?;
        if !engine
            .namespace_snapshot(&repository.main_branch_id)?
            .entries()
            .iter()
            .any(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == file)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::FileNotFound,
                "file is absent from the current owning namespace",
            ));
        }
        Some(if historical_inputs.is_some() {
            // Same-mutation helpers cannot be referenced by an existing current
            // composition. No frozen unrelated directory tree is imported.
            Vec::new()
        } else {
            view.file_merge_directories(state, &repository.repository_id, file, &branches)?
        })
    } else {
        None
    };
    let mut heads = Vec::new();
    let mut survivor_entries = None;
    for input in inputs {
        let branch = state
            .branches
            .get(&input.branch_id)
            .filter(|branch| {
                branch.file_id.as_deref() == Some(file)
                    && branch.history_tree_id == body.history_tree_id
            })
            .ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::FileBranchNotFound,
                    "merge branch does not belong to this file",
                )
            })?;
        if branch.state != BranchState::Active
            || state.file_branch_aliases.contains_key(&input.branch_id)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::FileBranchConflict,
                "merged branches cannot be merged or written again",
            ));
        }
        let head = super::file_nodes::branch_head(state, &input.branch_id)?;
        if let Some(view) = engine.admitted_current_namespaces()? {
            let current;
            let entries = if let Some(historical) = historical_inputs {
                historical.entries(branch)?
            } else {
                view.require_branch_write(state, &input.branch_id)?;
                current = engine.namespace_snapshot(&input.branch_id)?;
                current.entries()
            };
            let entry = entries
                .iter()
                .find(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == file)
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::FileNotFound,
                        "merge input is absent from its current namespace",
                    )
                })?;
            if super::file_nodes::node_version(state, file, &head)? != entry.version_id {
                return Err(Error::new(
                    storage_contract::ErrorCode::FileBranchConflict,
                    "merge input node differs from its current occurrence",
                ));
            }
            if input.branch_id == survivor {
                survivor_entries = Some(entries.to_vec());
            }
        }
        if head != input.expected_node_id {
            return Err(Error::new(
                storage_contract::ErrorCode::FileBranchConflict,
                "merge branch head changed; review all inputs again",
            ));
        }
        heads.push(head);
    }
    if !branches.contains(survivor) {
        return Err(Error::new(
            storage_contract::ErrorCode::InvalidRequest,
            "the surviving branch must be an explicit input",
        ));
    }
    // A merge adopts real bytes, including when the chosen node is historical.
    // Replica admission excludes retired resources before any target access.
    engine.valid_replica(&version)?;
    let reused = matches!(policy, MergeResultPolicy::ReuseChosenInput)
        && heads.iter().any(|head| head == chosen_node);
    let mut parents = heads.iter().cloned().collect::<BTreeSet<_>>();
    parents.insert(chosen_node.into());
    let result = if reused {
        chosen_node.to_owned()
    } else {
        new_id("fmeta")?
    };
    let edges = if reused {
        parents
            .into_iter()
            .filter(|parent| parent != &result && !reaches(state, parent, &result))
            .collect::<Vec<_>>()
    } else {
        parents.into_iter().collect::<Vec<_>>()
    };
    if !reused {
        state.file_metadata_nodes.insert(
            result.clone(),
            FileMetadataNode {
                node_id: result.clone(),
                history_tree_id: body.history_tree_id.clone(),
                file_id: file.into(),
                version_id: version.clone(),
                parent_node_ids: edges.clone(),
                branch_id: survivor.into(),
                kind: FileMetadataKind::Merge,
                operation_id: operation,
                author: principal.into(),
                sequence: state.sequence,
                created_at: now()?,
            },
        );
    }
    let merge_id = new_id("fmerge")?;
    state.file_branch_merges.insert(
        merge_id.clone(),
        FileBranchMerge {
            merge_id: merge_id.clone(),
            history_tree_id: body.history_tree_id.clone(),
            file_id: file.into(),
            input_branch_ids: inputs.iter().map(|input| input.branch_id.clone()).collect(),
            input_node_ids: heads,
            chosen_node_id: chosen_node.into(),
            result_node_id: result.clone(),
            surviving_branch_id: survivor.into(),
            reused_node: reused,
            parent_node_ids: edges,
            operation_id: operation,
            sequence: state.sequence,
        },
    );
    let publication = crate::business::file_publication::NodePublication {
        file,
        matching_branches: &branches,
        matching_node: None,
        target_branch: survivor,
        target_node: &result,
        principal,
        operation,
        maximum: engine.config.max_entries,
    };
    let affected = if let Some(directories) = projected {
        advance_current_merge_branch(
            state,
            &publication,
            survivor_entries.expect("invariant: current merge survivor input was admitted"),
        )?;
        publish_current_merge(engine, state, directories, publication)?
    } else {
        super::file_publication::advance_branch_node(
            state, survivor, &result, true, principal, operation,
        )?;
        super::file_publication::publish_node(state, publication)?
    };
    for branch in branches.iter().filter(|branch| branch.as_str() != survivor) {
        state
            .file_branch_aliases
            .insert(branch.clone(), survivor.into());
        let branch = state
            .branches
            .get_mut(branch)
            .expect("checked merge branch");
        branch.state = BranchState::Merged;
        branch.generation = branch.generation.checked_add(1).ok_or_else(|| {
            Error::new(
                storage_contract::ErrorCode::ResourceLimit,
                "branch generation exhausted",
            )
        })?;
    }
    // Historical aliases remain frozen, while their writable
    // ownership target follows a later explicit owner merge.
    let aliases = state
        .file_branch_aliases
        .iter()
        .filter(|(_, target)| branches.contains(*target) && target.as_str() != survivor)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for alias in aliases {
        *state
            .file_branch_aliases
            .get_mut(&alias)
            .expect("invariant: selected alias is still present") = survivor.into();
    }
    Ok(
        json!({"file_id":file,"merge_id":merge_id,"history_tree_id":body.history_tree_id,
                    "branch_id":survivor,"surviving_branch_id":survivor,"node_id":result,"version_id":version,
                    "chosen_node_id":chosen_node,"reused_node":reused,"directory_branch_ids":affected,
                    "merged_branch_ids":branches.into_iter().filter(|branch|branch!=survivor).collect::<Vec<_>>()}),
    )
}
