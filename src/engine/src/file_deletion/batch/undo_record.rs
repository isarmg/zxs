//! Canonical persistent encoding and admission for original batch restoration.
use super::undo_error::BatchUndoError;
use super::{MAX_RESULT_BYTES, UNDO, admit};
use crate::{Error, FileDeletionNode, OperationId, OperationResult, Result, State};
use data_serialization::Serialize;

#[derive(Serialize)]
struct Input {
    deletion_operation_id: OperationId,
}
#[derive(Serialize)]
struct Outcome<'a> {
    deletion_operation_id: OperationId,
    removed_deletion_nodes: &'a [FileDeletionNode],
}
fn invalid() -> Error {
    Error::BatchUndo(BatchUndoError::InvalidReceipt)
}
pub(super) fn encoded_size(value: &impl Serialize) -> Result<usize> {
    json_codec::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|source| Error::BatchUndo(BatchUndoError::Encoding(source)))
}
pub(super) fn input(operation: OperationId) -> Result<json_codec::Value> {
    json_codec::to_value(Input {
        deletion_operation_id: operation,
    })
    .map_err(|source| Error::BatchUndo(BatchUndoError::Encoding(source)))
}
pub(super) fn outcome(
    operation: OperationId,
    nodes: &[FileDeletionNode],
) -> Result<json_codec::Value> {
    let outcome = Outcome {
        deletion_operation_id: operation,
        removed_deletion_nodes: nodes,
    };
    if encoded_size(&outcome)? > MAX_RESULT_BYTES - 1024 {
        return Err(Error::BatchUndo(BatchUndoError::ResponseBudget));
    }
    json_codec::to_value(outcome)
        .map_err(|source| Error::BatchUndo(BatchUndoError::Encoding(source)))
}

pub(in crate::file_deletion) fn admit_undo(
    state: &State,
    receipt: &OperationResult,
) -> Result<Vec<FileDeletionNode>> {
    if receipt.kind != UNDO || encoded_size(&receipt.result)? > MAX_RESULT_BYTES {
        return Err(invalid());
    }
    let operation: OperationId = receipt
        .result
        .get("deletion_operation_id")
        .and_then(json_codec::Value::as_str)
        .ok_or_else(invalid)?
        .parse()
        .map_err(Error::Contract)?;
    crate::file_deletion::publication::admit_envelope(state, receipt, &input(operation)?, 5)?;
    let original = state.operations.get(&operation).ok_or_else(invalid)?;
    if original.operation_id != operation || original.sequence >= receipt.sequence {
        return Err(invalid());
    }
    let evidence = admit(state, original)?;
    let values = receipt
        .result
        .get("removed_deletion_nodes")
        .and_then(json_codec::Value::as_array)
        .ok_or_else(invalid)?;
    if values.len() != evidence.nodes.len() {
        return Err(invalid());
    }
    let nodes: Vec<FileDeletionNode> =
        json_codec::from_value(json_codec::Value::Array(values.clone()))
            .map_err(|source| Error::BatchUndo(BatchUndoError::Encoding(source)))?;
    if nodes != evidence.nodes {
        return Err(invalid());
    }
    Ok(nodes)
}
