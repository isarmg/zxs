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
    let current = engine.admitted_current_namespaces()?;
    if let Some(view) = current {
        view.require_branch_write(state, &repository.main_branch_id)?;
    } else if state.branches.get(&repository.main_branch_id)
        .is_none_or(|branch| branch.state != BranchState::Active)
    {
        return Err(Error::new(ErrorCode::BranchNotActive, "ordinary FILE namespace is not writable"));
    }
    if !engine.namespace_tree(&repository.main_branch_id)?.visible_entries()
        .any(|entry| entry.kind == NamespaceEntryKind::File && entry.file_id == file)
    {
        return Err(Error::new(ErrorCode::FileNotFound, "file is absent from the current owning namespace"));
    }
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
        if let Some(view) = current {
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
        target_branch: survivor,
        target_node: &result,
        principal,
        operation,
        maximum: engine.config.max_entries,
    };
    if let Some(entries) = survivor_entries {
        advance_current_merge_branch(state, &publication, entries)?;
    } else {
        super::file_publication::advance_branch_node(
            state, survivor, &result, true, principal, operation,
        )?;
    }
    super::file_publication::publish_node(engine, state, repository, &publication)?;
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
                    "chosen_node_id":chosen_node,"reused_node":reused,
                    "merged_branch_ids":branches.into_iter().filter(|branch|branch!=survivor).collect::<Vec<_>>()}),
    )
}
