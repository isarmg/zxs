//! Merge children retain their source and adoption proof after node labels retire.
use crate::business::file_tags::invalid;
use crate::engine::CurrentNamespaces;
use crate::{BranchState, Error, ErrorCode, FileMetadataKind, FileMetadataNode, Result, State};
use storage_contract::MapChanges;

impl CurrentNamespaces {
    pub(super) fn verify_merge_metadata(&self, previous: &State, next: &State) -> Result<()> {
        verify_metadata_sources(previous, next)
    }
}

fn verify_metadata_sources(previous: &State, next: &State) -> Result<()> {
    let inspect = |id: &String| -> Result<()> {
        let before = previous.file_metadata_nodes.get(id);
        let after = next.file_metadata_nodes.get(id);
        if before == after {
            return Ok(());
        }
        if before.is_some() {
            return Err(invalid());
        }
        let node = after.ok_or_else(invalid)?;
        if node.node_id != *id {
            return Err(invalid());
        }
        match node.kind {
            FileMetadataKind::Merge => verify_merge_child(previous, next, node)?,
        }
        Ok(())
    };
    match next
        .file_metadata_nodes
        .changes_since(&previous.file_metadata_nodes)
    {
        MapChanges::Keys(ids) => ids.iter().try_for_each(inspect),
        MapChanges::All => {
            for id in previous
                .file_metadata_nodes
                .keys()
                .chain(next.file_metadata_nodes.keys())
            {
                inspect(id)?;
            }
            Ok(())
        }
    }
}

fn verify_merge_child(previous: &State, next: &State, node: &FileMetadataNode) -> Result<()> {
    let merge = next
        .file_branch_merges
        .values()
        .find(|merge| {
            !previous.file_branch_merges.contains_key(&merge.merge_id)
                && !merge.reused_node
                && merge.result_node_id == node.node_id
                && merge.operation_id == node.operation_id
                && merge.surviving_branch_id == node.branch_id
                && merge.history_tree_id == node.history_tree_id
                && merge.file_id == node.file_id
                && merge.parent_node_ids == node.parent_node_ids
        })
        .ok_or_else(invalid)?;
    let receipt = next
        .operations
        .get(&node.operation_id)
        .ok_or_else(invalid)?;
    let principal = next
        .operation_principals
        .get(&node.operation_id)
        .ok_or_else(invalid)?;
    let branch = next.branches.get(&node.branch_id).ok_or_else(invalid)?;
    let commit = next
        .commits
        .get(&branch.head_commit_id)
        .ok_or_else(invalid)?;
    if previous.operations.contains_key(&node.operation_id)
        || !matches!(
            receipt.kind.as_str(),
            "file.branches.merge" | "file.history.merge"
        )
        || receipt.operation_id != node.operation_id
        || receipt.sequence != next.sequence
        || merge.sequence != next.sequence
        || node.sequence != next.sequence
        || node.author != *principal
        || branch.state != BranchState::Active
        || branch.file_node_id.as_deref() != Some(node.node_id.as_str())
        || previous
            .branches
            .get(&node.branch_id)
            .is_some_and(|old| old.head_commit_id == branch.head_commit_id)
        || previous.commits.contains_key(&branch.head_commit_id)
        || commit.kind != "file_metadata"
        || commit.provenance.as_deref() != Some(node.node_id.as_str())
        || commit.operation_id != node.operation_id
        || commit.sequence != next.sequence
        || commit.author != *principal
        || receipt.kind == "file.branches.merge"
            && merge.input_node_ids.contains(&merge.chosen_node_id)
    {
        return Err(invalid());
    }
    let version = checked_node_version(
        previous,
        &node.file_id,
        &merge.chosen_node_id,
        &node.history_tree_id,
    )
    .map_err(|source| source.context(ErrorCode::PublicationInvalid, "invalid file merge source"))?;
    if node.version_id != version {
        return Err(invalid());
    }
    Ok(())
}

fn checked_node_version(state: &State, file: &str, node: &str, history: &str) -> Result<String> {
    let (id, owner, source_history, version) = match (
        state.versions.get(node),
        state.file_path_nodes.get(node),
        state.file_metadata_nodes.get(node),
    ) {
        (Some(body), None, None) => (
            &body.version_id,
            &body.file_id,
            &body.history_tree_id,
            &body.version_id,
        ),
        (None, Some(path), None) => (
            &path.node_id,
            &path.file_id,
            &path.history_tree_id,
            &path.version_id,
        ),
        (None, None, Some(metadata)) => (
            &metadata.node_id,
            &metadata.file_id,
            &metadata.history_tree_id,
            &metadata.version_id,
        ),
        (None, None, None) => {
            return Err(Error::new(
                ErrorCode::FileNodeNotFound,
                "tag source node is missing",
            ));
        }
        _ => return Err(invalid()),
    };
    if owner != file || source_history != history {
        return Err(Error::new(
            ErrorCode::FileNodeNotFound,
            "tag source node escapes its file or history",
        ));
    }
    let body = state.versions.get(version).ok_or_else(invalid)?;
    if id != node
        || body.version_id != *version
        || body.file_id != file
        || body.history_tree_id != history
    {
        return Err(invalid());
    }
    Ok(version.clone())
}
