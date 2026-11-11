//! Exact native batch evidence. One operation owns several distinct file Ds;
//! it does not manufacture a file.delete operation for each member.
use super::*;
use crate::DirectoryBatchKind;
use data_serialization::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::file_deletion) struct Input {
    pub repository_id: String,
    pub request: DirectoryNamespaceBatch,
}

pub(in crate::file_deletion) struct Evidence {
    pub input: Input,
    pub nodes: Vec<FileDeletionNode>,
    pub sources: Vec<FrozenSource>,
}

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "batch deletion differs from its exact retained request",
    )
}

fn inherits(state: &State, branch: &str, ancestor: &str) -> bool {
    let mut current = branch;
    for _ in 0..state.directory_branches.len() {
        if current == ancestor {
            return true;
        }
        let Some(parent) = state
            .directory_branches
            .get(current)
            .and_then(|branch| branch.parent_branch_id.as_deref())
        else {
            return false;
        };
        current = parent;
    }
    false
}

fn requested_files(
    state: &State,
    input: &Input,
    sources: &[FrozenSource],
) -> Result<BTreeMap<String, String>> {
    HistoryRepositoryId::parse(&input.repository_id)?;
    if input.request.kind != DirectoryBatchKind::Delete
        || input.request.items.is_empty()
        || input.request.items.len() > MAX_FILES
        || input.request.items.len() != sources.len()
    {
        return Err(invalid());
    }
    let mut files = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for (item, frozen) in input.request.items.iter().zip(sources) {
        if item.target.is_some() {
            return Err(invalid());
        }
        let source = &item.source;
        let path = name_policy::PortablePath::parse(&source.path)?;
        let branch = state
            .directory_branches
            .get(&source.directory_branch_id)
            .ok_or_else(invalid)?;
        let tree = state
            .directory_trees
            .get(&frozen.tree_id)
            .ok_or_else(invalid)?;
        let baseline = state
            .directory_trees
            .get(&source.expected_tree_id)
            .ok_or_else(invalid)?;
        let baseline_owner = state
            .directory_branches
            .get(&baseline.directory_branch_id)
            .ok_or_else(invalid)?;
        let owner = state
            .directory_branches
            .get(&tree.directory_branch_id)
            .ok_or_else(invalid)?;
        if branch.repository_id != input.repository_id
            || owner.repository_id != input.repository_id
            || owner.directory_id != branch.directory_id
            || baseline_owner.repository_id != input.repository_id
            || baseline_owner.directory_id != branch.directory_id
            || !inherits(
                state,
                &branch.directory_branch_id,
                &tree.directory_branch_id,
            )
            || !seen.insert((
                &source.directory_branch_id,
                &source.entry_id,
                path.key().to_owned(),
            ))
        {
            return Err(invalid());
        }
        let entry = tree
            .entries
            .iter()
            .find(|entry| {
                entry.entry_id == source.entry_id
                    && entry.kind == crate::NamespaceEntryKind::File
                    && entry.file_id == frozen.file_id
            })
            .ok_or_else(invalid)?;
        // The immutable selected tree supplies identity; the request supplies
        // its then-current path, which may have been rebased by a directory move.
        files
            .entry(entry.file_id.clone())
            .or_insert_with(|| path.key().to_owned());
    }
    Ok(files)
}

pub(in crate::file_deletion) fn admit(
    state: &State,
    receipt: &OperationResult,
) -> Result<Evidence> {
    if receipt.kind != KIND || json_codec::to_vec(&receipt.result)?.len() > MAX_RESULT_BYTES {
        return Err(invalid());
    }
    let input_value = receipt.result.get("input").ok_or_else(invalid)?;
    let principal = super::super::publication::admit_envelope(state, receipt, input_value, 6)?;
    let input = Input::deserialize(input_value)?;
    let source_record = receipt.result.get("sources").ok_or_else(invalid)?;
    let source_values = source_record.as_array().ok_or_else(invalid)?;
    if source_values.len() > MAX_FILES {
        return Err(invalid());
    }
    let sources = Vec::<FrozenSource>::deserialize(source_record)?;
    let expected = requested_files(state, &input, &sources)?;
    let values = receipt
        .result
        .get("deletion_nodes")
        .and_then(json_codec::Value::as_array)
        .ok_or_else(invalid)?;
    if values.is_empty() || values.len() > MAX_FILES || values.len() != expected.len() {
        return Err(invalid());
    }
    let repository = state
        .repositories
        .get(&input.repository_id)
        .ok_or_else(invalid)?;
    let mut nodes = Vec::with_capacity(values.len());
    let mut ids = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut frontier = (0, 0);
    for value in values {
        let node = FileDeletionNode::deserialize(value)?;
        super::super::publication::admit_node_shape(&node)?;
        require_frontier((node.branch_heads.len(), node.parent_node_ids.len()))?;
        frontier.0 += node.branch_heads.len();
        frontier.1 += node.parent_node_ids.len();
        require_frontier(frontier)?;
        if node.repository_id != input.repository_id
            || node.history_tree_id != repository.history_tree_id
            || node.sequence != receipt.sequence
            || node.operation_id != receipt.operation_id
            || node.author != principal
            || expected.get(&node.file_id).map(String::as_str) != Some(node.path.key())
            || !ids.insert(node.node_id.clone())
            || !files.insert(node.file_id.clone())
        {
            return Err(invalid());
        }
        nodes.push(node);
    }
    if nodes.iter().map(|node| &node.file_id).ne(expected.keys()) {
        return Err(invalid());
    }
    Ok(Evidence {
        input,
        nodes,
        sources,
    })
}

pub(in crate::file_deletion) fn verify(
    previous: &State,
    next: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
    receipt: &OperationResult,
    changed: &BTreeSet<String>,
) -> Result<()> {
    let evidence = admit(next, receipt)?;
    if receipt.sequence != next.sequence || previous.operations.contains_key(&receipt.operation_id)
    {
        return Err(invalid());
    }
    let captured = capture(
        previous,
        current,
        &evidence.input.repository_id,
        &evidence.input.request,
        MAX_FILES,
    )?;
    if captured.targets.keys().ne(changed.iter())
        || captured.targets.len() != evidence.nodes.len()
        || captured.sources != evidence.sources
    {
        return Err(invalid());
    }
    for node in &evidence.nodes {
        if previous.file_deletions.contains_key(&node.file_id)
            || next.file_deletions.get(&node.file_id) != Some(node)
            || captured.targets.get(&node.file_id) != Some(&node.path)
        {
            return Err(invalid());
        }
        super::super::publication::verify_frontier(previous, current, node)?;
    }
    Ok(())
}

pub(in crate::file_deletion) fn node_for(
    receipt: &OperationResult,
    file: &str,
) -> Result<FileDeletionNode> {
    let field = match receipt.kind.as_str() {
        KIND => "deletion_nodes",
        UNDO => "removed_deletion_nodes",
        _ => return Err(invalid()),
    };
    let nodes = receipt
        .result
        .get(field)
        .and_then(json_codec::Value::as_array)
        .ok_or_else(invalid)?;
    if nodes.is_empty() || nodes.len() > MAX_FILES {
        return Err(invalid());
    }
    let mut matches = nodes
        .iter()
        .filter(|node| node["file_id"].as_str() == Some(file));
    let node = matches.next().ok_or_else(invalid)?;
    if matches.next().is_some() {
        return Err(invalid());
    }
    Ok(FileDeletionNode::deserialize(node)?)
}
