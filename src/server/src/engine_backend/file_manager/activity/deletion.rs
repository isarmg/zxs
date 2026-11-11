//! Native lifecycle receipts retain their original paths after exact-node undo.
use crate::engine_backend::{
    file_manager::activity::{
        ActivityRows, PendingDetail, PendingEvent, Timestamp, optional_field, receipt_field, record_event,
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

// Native admission owns the complete receipt. This read projection borrows only
// displayed fields so a directory's member map is not cloned for its group row.
#[derive(data_serialization::Deserialize)]
struct DirectoryActivity<'a> {
    node_id: &'a str,
    history_tree_id: &'a str,
    path: &'a str,
    directory_branch_id: &'a str,
    source_tree_id: &'a str,
    #[serialization(with = "storage_contract::u64_string")]
    created_at: u64,
}

#[derive(data_serialization::Deserialize)]
struct DeletedDirectory<'a> {
    #[serialization(borrow)]
    directory_deletion: DirectoryActivity<'a>,
}

#[derive(data_serialization::Deserialize)]
struct RestoredDirectory<'a> {
    #[serialization(borrow)]
    removed_directory_deletion: DirectoryActivity<'a>,
}

fn record_directory<'a>(
    engine: &'a Engine,
    events: &mut ActivityRows<'a>,
    operation: &'a OperationResult,
) -> Result<bool, ApiError> {
    let (action, node) = match operation.kind.as_str() {
        "directory.delete" => (
            Action::Delete,
            receipt_field::<DeletedDirectory<'_>>(&operation.result)?.directory_deletion,
        ),
        "directory.delete.undo" => (
            Action::Undo,
            receipt_field::<RestoredDirectory<'_>>(&operation.result)?.removed_directory_deletion,
        ),
        _ => return Ok(false),
    };
    let repository = repo_for_history(engine, &node.history_tree_id)?;
    let path = if repository.file_manager_root {
        node.path.to_owned()
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
            file_id: String::new(),
            message: String::new(),
            detail: PendingDetail::Captured(ActivityDetailSnapshot::DirectoryOperation {
                version_id: None,
                directory_branch_id: node.directory_branch_id.into(),
                head_tree_id: Some(node.source_tree_id.into()),
            }),
        },
    );
    Ok(true)
}

pub(super) fn record_operation<'a>(
    engine: &'a Engine,
    events: &mut ActivityRows<'a>,
    operation: &'a OperationResult,
) -> Result<bool, ApiError> {
    if record_directory(engine, events, operation)? {
        return Ok(true);
    }
    let Some((action, nodes)) = nodes(operation)? else {
        return Ok(false);
    };
    for node in nodes {
        record_node(engine, events, operation, action, node)?;
    }
    Ok(true)
}
