//! Restore one complete native batch through the ordinary publication owner.
pub use super::undo_error::{BatchUndoActor, BatchUndoError};
use super::undo_record;
use super::{KIND, MAX_RESULT_BYTES, UNDO, admit, receipt};
use crate::file_deletion::{directory, lifecycle, preflight};
use crate::{Engine, Error, FileDeletionNode, OperationId, OperationResult, Result, State};
use std::collections::BTreeSet;
use storage_contract::file_deletion_batch::FileDeletionBatchUndoPreview;

pub struct UndoFileDeletionBatchRequest<'a> {
    pub deletion_operation_id: OperationId,
    pub principal: BatchUndoActor<'a>,
    pub operation_id: OperationId,
}

fn invalid() -> Error {
    Error::BatchUndo(BatchUndoError::InvalidReceipt)
}

fn original(state: &State, operation: OperationId) -> Result<receipt::Evidence> {
    let receipt = state
        .operations
        .get(&operation)
        .ok_or_else(|| Error::BatchUndo(BatchUndoError::OperationMissing(operation)))?;
    if receipt.kind != KIND {
        return Err(Error::BatchUndo(BatchUndoError::NotDeletionBatch(
            operation,
        )));
    }
    if receipt.operation_id != operation {
        return Err(invalid());
    }
    admit(state, receipt)
}

struct RestorableBatch {
    nodes: Vec<FileDeletionNode>,
}

fn restorable(
    state: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
    nodes: Vec<FileDeletionNode>,
) -> Result<RestorableBatch> {
    for node in &nodes {
        if state.file_deletions.get(&node.file_id) != Some(node) {
            return Err(Error::BatchUndo(BatchUndoError::MemberChanged));
        }
        directory::require_individual_undo(state, &node.file_id)?;
        preflight::require_undo_scope(state, current, node)?;
    }
    Ok(RestorableBatch { nodes })
}

fn eligible(engine: &Engine, operation: OperationId) -> Result<RestorableBatch> {
    let evidence = original(engine.state(), operation)?;
    let repository = engine.repository_for_history(&evidence.nodes[0].history_tree_id)?;
    lifecycle::require_writable(engine, repository)?;
    let current = engine.admitted_current_namespaces()?;
    if let Some(view) = current {
        view.require_operation(UNDO)?;
    }
    restorable(engine.state(), current, evidence.nodes)
}

impl Engine {
    /// Capture the complete original batch under the current writable owners.
    pub fn preflight_file_deletion_batch_undo(
        &self,
        deletion_operation_id: OperationId,
        _actor: BatchUndoActor<'_>,
    ) -> Result<FileDeletionBatchUndoPreview> {
        self.require_write()?;
        let preview = FileDeletionBatchUndoPreview {
            deletion_operation_id,
            deletion_nodes: eligible(self, deletion_operation_id)?.nodes,
        };
        if undo_record::encoded_size(&preview)? > MAX_RESULT_BYTES {
            return Err(Error::BatchUndo(BatchUndoError::ResponseBudget));
        }
        Ok(preview)
    }

    /// Restore the complete original batch in one publication after current-owner admission.
    pub fn undo_file_deletion_batch(
        &mut self,
        request: UndoFileDeletionBatchRequest<'_>,
    ) -> Result<OperationResult> {
        let UndoFileDeletionBatchRequest {
            deletion_operation_id,
            principal,
            operation_id,
        } = request;
        self.mutate(
            UNDO,
            principal.as_str(),
            operation_id,
            undo_record::input(deletion_operation_id)?,
            |engine, next, _| {
                let batch = eligible(engine, deletion_operation_id)?;
                let result = undo_record::outcome(deletion_operation_id, &batch.nodes)?;
                for node in batch.nodes {
                    next.file_deletions.remove(&node.file_id).expect(
                        "invariant: every original batch deletion was admitted in this draft",
                    );
                }
                Ok(result)
            },
        )
    }
}

pub(in crate::file_deletion) use super::undo_record::admit_undo;

pub(in crate::file_deletion) fn verify_undo(
    previous: &State,
    next: &State,
    current: Option<&crate::engine::CurrentNamespaces>,
    receipt: &OperationResult,
    changed: &BTreeSet<String>,
) -> Result<()> {
    let nodes = admit_undo(next, receipt)?;
    if receipt.sequence != next.sequence
        || previous.operations.contains_key(&receipt.operation_id)
        || nodes.iter().map(|node| &node.file_id).ne(changed.iter())
    {
        return Err(invalid());
    }
    let batch = restorable(previous, current, nodes)?;
    if batch
        .nodes
        .iter()
        .any(|node| next.file_deletions.contains_key(&node.file_id))
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/file_deletion_batch_undo_tests.rs"]
mod tests;
