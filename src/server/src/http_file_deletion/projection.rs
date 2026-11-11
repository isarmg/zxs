use crate::ApiError;
use storage_contract::file_deletion_review::Summary;
use storage_contract::{FileDeletionNode, OperationOutcome, Sequence, Timestamp};

pub(crate) fn summary(node: &FileDeletionNode) -> Summary {
    Summary {
        file_id: node.file_id.parse().expect("invariant: admitted file ID"),
        node_id: node.node_id.parse().expect("invariant: admitted deletion ID"),
        repository_id: node
            .repository_id
            .parse()
            .expect("invariant: admitted repository ID"),
        history_tree_id: node
            .history_tree_id
            .parse()
            .expect("invariant: admitted history ID"),
        path: node.path.clone(),
        operation_id: node.operation_id,
        author: node
            .author
            .parse()
            .expect("invariant: admitted deletion author"),
        sequence: Sequence::new(node.sequence),
        created_at: Timestamp::from_unix_seconds(node.created_at),
        branch_count: node.branch_heads.len() as u64,
        parent_count: node.parent_node_ids.len() as u64,
    }
}

pub(super) fn undo(mut outcome: OperationOutcome) -> Result<json_codec::Value, ApiError> {
    if outcome.kind != "file.delete.undo" {
        return Err(ApiError::invalid("operation is not a file deletion undo"));
    }
    // Keep every original envelope and result field except the large audit node.
    if let Some(result) = outcome.result.as_object_mut()
        && let Some(node) = result.remove("removed_deletion_node")
    {
        let node: FileDeletionNode = json_codec::from_value(node)
            .expect("invariant: admitted undo receipt contains its original deletion node");
        result.insert(
            "removed_deletion_summary".into(),
            json_codec::to_value(summary(&node)).expect("invariant: deletion summary serializes"),
        );
    }
    crate::operation_wire::operation_wire(outcome)
}

#[cfg(test)]
#[path = "../../../../test/server/unit/deletion_review_projection.rs"]
mod tests;
