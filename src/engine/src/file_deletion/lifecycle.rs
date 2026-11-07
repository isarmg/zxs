//! Metadata-only deletion and exact-node cancellation use ordinary publication.
use super::*;
use crate::{BranchState, FileDeletionNode, OperationId, OperationResult};
use json_codec::json;

/// A deletion binds the current stable file at the supplied lookup location.
/// The actual complete branch frontier is captured under the Engine owner.
pub struct DeleteFileRequest<'a> {
    pub target: FileDeletionTarget<'a>,
    pub principal: &'a str,
    pub operation_id: OperationId,
}

/// Cancellation is tied to the exact current deletion, not only to the file.
pub struct UndoFileDeletionRequest<'a> {
    pub file_id: &'a HistoryFileId,
    pub deletion_node_id: &'a str,
    pub principal: &'a str,
    pub operation_id: OperationId,
}

pub(crate) fn frozen() -> Error {
    Error::new(
        ErrorCode::BranchNotActive,
        "the file is soft deleted; remove its current deletion node before changing branches, nodes or file selection",
    )
}

pub(super) fn require_writable(engine: &Engine, repository: &Repository) -> Result<()> {
    let branch = &engine.state().branches[&repository.main_branch_id];
    if branch.state != BranchState::Active {
        return Err(Error::new(
            ErrorCode::BranchNotActive,
            "file namespace is not writable",
        ));
    }
    if let Some(view) = engine.admitted_current_namespaces()? {
        view.require_branch_write(engine.state(), &repository.main_branch_id)?;
    }
    Ok(())
}

impl Engine {
    /// Publish one bodyless deletion node and freeze this stable file only.
    ///
    /// The trusted caller authenticates the actor. The same publication owns
    /// lifecycle, history projection, and ordinary operation idempotency. No
    /// content version, branch HEAD, container name or content byte is changed.
    /// The logical location remains reserved until this deletion is undone.
    pub fn soft_delete_file(&mut self, request: DeleteFileRequest<'_>) -> Result<OperationResult> {
        let DeleteFileRequest {
            target,
            principal,
            operation_id,
        } = request;
        self.mutate(
            "file.delete", principal, operation_id,
            json!({"repository_id": target.repository_id.as_str(), "file_id": target.file_id.as_str(), "path": target.path.key()}),
            |engine, state, _| {
                if state.file_deletions.contains_key(target.file_id.as_str()) {
                    return Err(frozen());
                }
                let capture = engine.file_deletion_preflight(target, &HistoryWindowCancellation::default())?;
                require_writable(engine, capture.repository())?;
                let selected = capture.occurrences().iter()
                    .find(|entry| entry.lookup_key() == target.path.key())
                    .expect("invariant: preflight admitted the exact requested occurrence");
                let node = FileDeletionNode {
                    node_id: crate::new_id("fdel")?,
                    repository_id: capture.repository().repository_id.clone(),
                    history_tree_id: capture.repository().history_tree_id.clone(),
                    file_id: target.file_id.as_str().into(),
                    path: selected.path.clone(),
                    branch_heads: capture.branches().iter()
                        .map(|head| (head.branch().branch_id.clone(), head.node_id().into()))
                        .collect(),
                    parent_node_ids: capture.parent_node_ids().to_vec(),
                    operation_id,
                    author: principal.into(),
                    sequence: state.sequence,
                    created_at: crate::business::now()?,
                };
                state.file_deletions.insert(node.file_id.clone(), node.clone());
                Ok(json!({"deletion_node": node}))
            },
        )
    }

    /// Remove the exact current deletion node. Its immutable operation receipt
    /// remains audit evidence; it is no longer part of the current file graph.
    pub fn undo_file_deletion(
        &mut self,
        request: UndoFileDeletionRequest<'_>,
    ) -> Result<OperationResult> {
        let UndoFileDeletionRequest {
            file_id,
            deletion_node_id,
            principal,
            operation_id,
        } = request;
        storage_contract::validate_id(deletion_node_id, "fdel")?;
        self.mutate(
            "file.delete.undo",
            principal,
            operation_id,
            json!({"file_id": file_id.as_str(), "deletion_node_id": deletion_node_id}),
            |engine, state, _| {
                let deletion = state
                    .file_deletions
                    .get(file_id.as_str())
                    .filter(|node| node.node_id == deletion_node_id)
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::FileNodeNotFound,
                            "current deletion node differs or is absent",
                        )
                    })?;
                let repository = engine.repository_for_history(&deletion.history_tree_id)?;
                require_writable(engine, repository)?;
                let node = state
                    .file_deletions
                    .remove(file_id.as_str())
                    .expect("invariant: exact current deletion was checked above");
                Ok(json!({"restored_file_id": file_id.as_str(), "removed_deletion_node": node}))
            },
        )
    }

    /// Current lifecycle only. This does not grant content or history access.
    pub fn file_deletion(&self, file_id: &str) -> Option<&FileDeletionNode> {
        self.state().file_deletions.get(file_id)
    }
}
