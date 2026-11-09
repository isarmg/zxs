//! Path-free retained D/undo audit. Imported rows are copied facts, never live
//! deletions or fabricated operation receipts. Live lifecycle admission is separate.
use crate::{Error, ErrorCode, OperationId, Result, State, state_projection::StateProjection};
use data_serialization::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
pub(crate) struct DeletionRow {
    pub node_id: String,
    pub history_tree_id: String,
    pub file_id: String,
    pub branch_heads: BTreeMap<String, String>,
    pub parent_node_ids: Vec<String>,
    pub operation_id: OperationId,
    pub author: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub created_at: u64,
}
impl From<&crate::FileDeletionNode> for DeletionRow {
    fn from(node: &crate::FileDeletionNode) -> Self {
        Self {
            node_id: node.node_id.clone(),
            history_tree_id: node.history_tree_id.clone(),
            file_id: node.file_id.clone(),
            branch_heads: node.branch_heads.clone(),
            parent_node_ids: node.parent_node_ids.clone(),
            operation_id: node.operation_id,
            author: node.author.clone(),
            sequence: node.sequence,
            created_at: node.created_at,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serialization(rename_all = "snake_case")]
pub(crate) enum DeletionAction {
    Delete,
    Undo,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct LifecycleRow {
    pub operation_id: OperationId,
    pub action: DeletionAction,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub author: String,
    pub node: DeletionRow,
}

fn invalid() -> Error {
    Error::new(
        ErrorCode::PublicationInvalid,
        "retained deletion audit is incomplete or has inconsistent ownership",
    )
}

fn direct(state: &State, id: OperationId, file: &str) -> Result<LifecycleRow> {
    let receipt = &state.operations[&id];
    let (action, key) = match receipt.kind.as_str() {
        "file.delete" | crate::business::FILE_HISTORY_MOVE_KIND => {
            (DeletionAction::Delete, "deletion_node")
        }
        "file.delete.undo" => (DeletionAction::Undo, "removed_deletion_node"),
        _ => return Err(invalid()),
    };
    let source: crate::FileDeletionNode =
        json_codec::from_value(receipt.result.get(key).ok_or_else(invalid)?.clone())?;
    let author = state.operation_principals.get(&id).ok_or_else(invalid)?;
    if receipt.operation_id != id
        || source.file_id != file
        || action == DeletionAction::Delete
            && (source.operation_id != id
                || source.sequence != receipt.sequence
                || source.author != *author)
    {
        return Err(invalid());
    }
    Ok(LifecycleRow {
        operation_id: id,
        action,
        sequence: receipt.sequence,
        author: author.clone(),
        node: (&source).into(),
    })
}

/// Flattened copied audit plus this file's own permanent lifecycle. Every copy
/// receipt holds its complete imported prefix, so this never chases a mutable
/// source file or recursively follows copy ancestry.
pub(crate) fn events(view: StateProjection<'_>, file: &str) -> Result<Vec<LifecycleRow>> {
    if view.owners().malformed_deletion_receipts()
        || view.owners().malformed_history_copy_receipts()
    {
        return Err(invalid());
    }
    let Some(rows) = view.owners().file_rows(file) else {
        return Ok(Vec::new());
    };
    if rows.history_copies.len() > 1 {
        return Err(invalid());
    }
    let state = view.state();
    let mut events = BTreeMap::new();
    let mut ids = BTreeSet::new();
    let mut insert = |event: LifecycleRow| -> Result<()> {
        if events.len() == 65_536 {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "retained deletion audit exceeds its event budget",
            ));
        }
        if !ids.insert(event.operation_id) || events.insert(event.sequence, event).is_some() {
            return Err(invalid());
        }
        Ok(())
    };
    for id in rows.history_copies.keys() {
        let receipt = &state.operations[id];
        if receipt.operation_id != *id {
            return Err(invalid());
        }
        if receipt.kind == crate::business::history_copy::RETAINED_KIND
            || receipt.kind == crate::business::history_copy::DIRECTORY_EXPORT_KIND
            || receipt.kind == crate::business::FILE_HISTORY_MOVE_KIND
        {
            let copied = crate::business::history_copy::retained::read(receipt, file)?;
            for event in copied.events {
                insert(event)?;
            }
        }
    }
    for id in rows.deletion_operations.keys() {
        insert(direct(state, *id, file)?)?;
    }
    Ok(events.into_values().collect())
}
