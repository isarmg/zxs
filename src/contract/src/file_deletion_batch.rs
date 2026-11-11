//! A retained batch operation identifies its complete immutable deletion set.
use crate::{FileDeletionNode, OperationId};
use data_serialization::{Deserialize, Serialize};

pub const UNDO_KIND: &str = "file.delete.batch.undo";
pub const MAX_RESPONSE_BYTES: usize = crate::client_protocol::MAX_FILE_DELETION_RESPONSE_BYTES / 2;

/// Complete original deletion members captured for the current review.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct FileDeletionBatchUndoPreview {
    pub deletion_operation_id: OperationId,
    pub deletion_nodes: Vec<FileDeletionNode>,
}

/// Identify the original deletion batch and this atomic restoration operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct UndoFileDeletionBatchBody {
    pub deletion_operation_id: OperationId,
    pub operation_id: OperationId,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/file_deletion_batch.rs"]
mod tests;
