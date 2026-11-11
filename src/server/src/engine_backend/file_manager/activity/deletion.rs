//! Native lifecycle receipts retain their original paths after exact-node undo.
use crate::engine_backend::{
    file_manager::activity::{
        ActivityRows, PendingDetail, PendingEvent, Timestamp, optional_field, record_event,
    },
    repo_for_history,
};
use crate::{ApiError, backend_activity::ActivityDetailSnapshot};
use storage_contract::{FileDeletionNode, OperationResult};
use storage_engine::Engine;

#[derive(Clone, Copy)]
enum Action {
    Delete,
    Undo,
}

enum NodeField {
    One(&'static str),
    Batch(&'static str),
}

// Cold native lifecycle admission requires these fields on every retained receipt.
fn nodes(operation: &OperationResult) -> Result<Option<(Action, Vec<FileDeletionNode>)>, ApiError> {
    let formats = [
        ("file.delete", Action::Delete, NodeField::One("deletion_node")),
        (
            "file.delete.undo",
            Action::Undo,
            NodeField::One("removed_deletion_node"),
        ),
        (
            "file.delete.batch",
            Action::Delete,
            NodeField::Batch("deletion_nodes"),
        ),
        (
            storage_contract::file_deletion_batch::UNDO_KIND,
            Action::Undo,
            NodeField::Batch("removed_deletion_nodes"),
        ),
    ];
    let Some((_, action, field)) = formats
        .into_iter()
        .find(|(kind, _, _)| operation.kind == *kind)
    else {
        return Ok(None);
    };
    let nodes = match field {
        NodeField::One(field) => optional_field::<FileDeletionNode>(operation.result.get(field))?
            .map(|node| vec![node]),
        NodeField::Batch(field) => {
            optional_field::<Vec<FileDeletionNode>>(operation.result.get(field))?
        }
    };
    Ok(Some((
        action,
        nodes.expect("invariant: native lifecycle admission retains the complete deletion set"),
    )))
}

fn record_node<'a>(
    engine: &'a Engine,
    events: &mut ActivityRows<'a>,
    operation: &'a OperationResult,
    action: Action,
    node: FileDeletionNode,
) -> Result<(), ApiError> {
    let repository = repo_for_history(engine, &node.history_tree_id)?;
    let path = if repository.file_manager_root {
        node.path.as_str().to_owned()
    } else {
        format!("{}/{}", repository.name, node.path)
    };
    let timestamp = match action {
        Action::Delete => Timestamp::Seconds(node.created_at),
        Action::Undo => Timestamp::Receipt(operation.result.get("timestamp")),
    };
    record_event(
        events,
        operation.sequence,
        format!("{}:{}", operation.operation_id, node.node_id),
        PendingEvent {
            timestamp,
            slot_id: repository.primary_slot_id.clone(),
            path,
            action: operation.kind.clone(),
            principal: engine
                .operation_owner(operation.operation_id)
                .map_err(crate::engine_backend::error)?,
            file_id: node.file_id,
            message: String::new(),
            detail: PendingDetail::Captured(ActivityDetailSnapshot::FileOperation {
                version_id: None,
                node_id: Some(node.node_id),
                branch_id: None,
            }),
        },
    );
    Ok(())
}

pub(super) fn record_operation<'a>(
    engine: &'a Engine,
    events: &mut ActivityRows<'a>,
    operation: &'a OperationResult,
) -> Result<bool, ApiError> {
    let Some((action, nodes)) = nodes(operation)? else {
        return Ok(false);
    };
    for node in nodes {
        record_node(engine, events, operation, action, node)?;
    }
    Ok(true)
}
