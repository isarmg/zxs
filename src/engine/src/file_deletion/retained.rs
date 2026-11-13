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
    /// Relocate an already deleted unit without inventing an undo/delete pair.
    Import,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
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

fn direct(
    view: crate::state_projection::lifecycles::LifecycleView<'_>,
    id: OperationId,
) -> Result<LifecycleRow> {
    let state = view.state();
    let file = view.file().as_str();
    let receipt = &state.operations[&id];
    if receipt.kind == storage_contract::file_relocation::OBJECT_MOVE_KIND {
        let movement = view.object_move(id)?;
        let (action, source) = movement.deletion(file)?;
        return direct_row(state, id, file, action, source);
    }
    let (action, key) = match receipt.kind.as_str() {
        "file.delete"
        | crate::business::FILE_HISTORY_MOVE_KIND
        | super::batch::KIND
        | super::directory::DELETE => (DeletionAction::Delete, "deletion_node"),
        "file.delete.undo" | super::batch::UNDO | super::directory::UNDO => {
            (DeletionAction::Undo, "removed_deletion_node")
        }
        _ => return Err(invalid()),
    };
    let source: crate::FileDeletionNode = if super::directory::is_kind(&receipt.kind) {
        super::directory::node_for(state, receipt, file)?
    } else if super::batch::is_kind(&receipt.kind) {
        super::batch::node_for(receipt, file)?
    } else if receipt.kind == crate::business::FILE_HISTORY_MOVE_KIND {
        crate::business::retained_file_history_move_deletion(receipt, file)?
    } else {
        json_codec::from_value(receipt.result.get(key).ok_or_else(invalid)?.clone())?
    };
    direct_row(state, id, file, action, &source)
}

fn direct_row(
    state: &State,
    id: OperationId,
    file: &str,
    action: DeletionAction,
    source: &crate::FileDeletionNode,
) -> Result<LifecycleRow> {
    let receipt = &state.operations[&id];
    let author = state.operation_principals.get(&id).ok_or_else(invalid)?;
    if receipt.operation_id != id
        || source.file_id != file
        || action != DeletionAction::Undo
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
        node: source.into(),
    })
}

/// Flattened copied audit plus this file's own permanent lifecycle. Every copy
/// receipt holds its complete imported prefix, so this never chases a mutable
/// source file or recursively follows copy ancestry.
pub(crate) fn events(view: StateProjection<'_>, file: &str) -> Result<Vec<LifecycleRow>> {
    let rows = view.owners().file_rows(file);
    admit_source(
        view.owners().malformed_deletion_receipts()
            || view.owners().malformed_history_copy_receipts(),
        rows.map_or(0, |rows| rows.history_copies.len()),
    )?;
    // Preserve the existing FILE adapter's malformed-before-absent precedence.
    // An absent key has no source, even when its caller supplied invalid text.
    if rows.is_none() {
        return Ok(Vec::new());
    }
    let file = storage_contract::history_window::HistoryFileId::parse(file)?;
    collect_events(view.lifecycle(&file), || Ok(()), Ok)
}

fn admit_source(malformed: bool, copies: usize) -> Result<()> {
    if malformed || copies > 1 {
        return Err(invalid());
    }
    Ok(())
}

/// The audit remains complete: cancellation returns an error, never a prefix.
pub(crate) fn events_from_source<T>(
    view: crate::state_projection::lifecycles::LifecycleView<'_>,
    mut checkpoint: impl FnMut() -> Result<()>,
    project: impl FnMut(LifecycleRow) -> Result<T>,
) -> Result<Vec<T>> {
    checkpoint()?;
    admit_source(view.malformed(), view.copies().map_or(0, |copies| copies.len()))?;
    collect_events(view, checkpoint, project)
}

fn collect_events<T>(
    view: crate::state_projection::lifecycles::LifecycleView<'_>,
    mut checkpoint: impl FnMut() -> Result<()>,
    mut project: impl FnMut(LifecycleRow) -> Result<T>,
) -> Result<Vec<T>> {
    let state = view.state();
    let file = view.file().as_str();
    let mut events = BTreeMap::new();
    let mut ids = BTreeSet::new();
    let mut insert = |event: LifecycleRow| -> Result<()> {
        checkpoint()?;
        if events.len() == storage_contract::file_lifecycle::MAX_AUDIT_EVENTS {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "retained deletion audit exceeds its event budget",
            ));
        }
        if !ids.insert(event.operation_id) || events.contains_key(&event.sequence) {
            return Err(invalid());
        }
        events.insert(event.sequence, project(event)?);
        Ok(())
    };
    for id in view.copies().into_iter().flat_map(|copies| copies.keys()) {
        let receipt = &state.operations[id];
        if receipt.operation_id != *id {
            return Err(invalid());
        }
        if receipt.kind == storage_contract::file_relocation::OBJECT_MOVE_KIND {
            let movement = view.object_move(*id)?;
            for event in &movement.retained(file)?.events {
                insert(event.clone())?;
            }
        } else if crate::business::history_copy::is_kind(&receipt.kind)
            || receipt.kind == crate::business::FILE_HISTORY_MOVE_KIND
        {
            let copied = crate::business::history_copy::retained::read(receipt, file)?;
            for event in copied.events {
                insert(event)?;
            }
        }
    }
    for id in view.deletions().into_iter().flat_map(|deletions| deletions.keys()) {
        insert(direct(view, *id)?)?;
    }
    Ok(events.into_values().collect())
}
