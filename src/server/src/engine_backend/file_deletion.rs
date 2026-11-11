//! The same core publication owns SMB deletion and management restoration.
use super::*;
use storage_contract::client_protocol::MAX_FILE_DELETION_FRONTIER_ENTRIES;
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

fn require_recoverable_frontier(branches: usize, parents: usize) -> Result<(), ApiError> {
    if branches > MAX_FILE_DELETION_FRONTIER_ENTRIES || parents > MAX_FILE_DELETION_FRONTIER_ENTRIES
    {
        return Err(ApiError::limit(
            "complete deletion frontier exceeds response record budget",
        ));
    }
    Ok(())
}

/// Called under the same engine owner lock as the ensuing new publication.
/// Both user entrances must admit the complete frontier that recovery can read.
fn admit_new_deletion(
    engine: &Engine,
    target: storage_engine::FileDeletionTarget<'_>,
) -> Result<(), ApiError> {
    let capture = engine
        .file_deletion_preflight(
            target,
            &storage_contract::history_window::HistoryWindowCancellation::default(),
        )
        .map_err(error)?;
    require_recoverable_frontier(capture.branches().len(), capture.parent_node_ids().len())
}

impl EngineBackend {
    pub(super) fn soft_delete_file_admitted(
        &self,
        identity: &Identity,
        command: crate::DeleteFile,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        // Existing receipts keep their original semantics and are never
        // re-admitted against today's location, frontier or response budget.
        if !engine
            .state()
            .operations
            .contains_key(&command.operation_id)
        {
            admit_new_deletion(
                &engine,
                storage_engine::FileDeletionTarget {
                    repository_id: &command.repository_id,
                    file_id: &command.file_id,
                    path: &command.path,
                },
            )?;
        }
        // Core checks the original canonical receipt before today's namespace.
        // A retry after undo must return that receipt, never delete the file again.
        engine
            .soft_delete_file(storage_engine::DeleteFileRequest {
                target: storage_engine::FileDeletionTarget {
                    repository_id: &command.repository_id,
                    file_id: &command.file_id,
                    path: &command.path,
                },
                principal: &current.principal,
                operation_id: command.operation_id,
            })
            .map_err(error)
    }

    pub(super) fn delete_current_file_admitted(
        &self,
        identity: &Identity,
        command: crate::DeleteCurrentFile,
    ) -> Result<Option<storage_contract::OperationResult>, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        if current
            .smb_binding
            .as_ref()
            .is_some_and(|binding| binding.branch_id != command.branch_id)
        {
            return Err(ApiError::forbidden());
        }
        let repository = repo_for_branch(&engine, &command.branch_id)?;
        if repository.main_branch_id != command.branch_id {
            return Ok(None);
        }
        let repository_id = HistoryRepositoryId::parse(&repository.repository_id)
            .map_err(|source| error(source.into()))?;
        let request = storage_engine::DeleteFileRequest {
            target: storage_engine::FileDeletionTarget {
                repository_id: &repository_id,
                file_id: &command.file_id,
                path: &command.path,
            },
            principal: &current.principal,
            operation_id: command.operation_id,
        };
        if engine
            .state()
            .operations
            .contains_key(&command.operation_id)
        {
            // The entry ID guards a new path resolution. A retained receipt is
            // checked by its canonical repository/file/path/actor before current
            // visibility, including after undo or another later deletion.
            return engine.soft_delete_file(request).map(Some).map_err(error);
        }
        let object = engine
            .locate_object(&command.branch_id, &command.path)
            .map_err(error)?
            .ok_or_else(|| {
                ApiError::new(
                    http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::FileNotFound,
                    "delete target is no longer visible",
                )
            })?;
        let entry = object.entry();
        if entry.kind != storage_contract::NamespaceEntryKind::File {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::IsDirectory,
                "unlink requires a file",
            ));
        }
        if entry.entry_id != command.expected_entry_id || entry.file_id != command.file_id.as_str()
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::WorkspaceChanged,
                "delete target identity changed",
            ));
        }
        // Legacy named namespaces still use structural deletion. An initialized
        // owner requires the same reversible lifecycle as a disk-root file.
        if !repository.file_manager_root && object.initialized_repository_id().is_none() {
            return Ok(None);
        }
        // SMB must not publish a D that the management recovery entrance cannot
        // read. Preserve the captured entry guard above and do not trim parents.
        admit_new_deletion(&engine, request.target)?;
        engine.soft_delete_file(request).map(Some).map_err(error)
    }

    pub(super) fn current_file_deletion_admitted(
        &self,
        identity: &Identity,
        file: &HistoryFileId,
    ) -> Result<Option<storage_contract::FileDeletionNode>, ApiError> {
        let (engine, current) = self.lock_current(identity)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        let node = engine.file_deletion(file.as_str());
        if let Some(node) = node {
            require_recoverable_frontier(node.branch_heads.len(), node.parent_node_ids.len())?;
        }
        Ok(node.cloned())
    }

    pub(super) fn file_deletions_admitted(
        &self,
        identity: &Identity,
        request: crate::ReadFileDeletions,
    ) -> Result<crate::FileDeletionsPage, ApiError> {
        let (engine, current) = self.lock_current(identity)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        if request
            .snapshot
            .as_deref()
            .is_some_and(|snapshot| snapshot != engine.listing_snapshot())
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::ListingChanged,
                "deleted files changed; restart listing",
            ));
        }
        let lower = request
            .after
            .as_ref()
            .map_or(std::ops::Bound::Unbounded, |after| {
                std::ops::Bound::Excluded(after.as_str())
            });
        let mut nodes = engine
            .state()
            .file_deletions
            .range::<str, _>((lower, std::ops::Bound::Unbounded));
        let entries = nodes
            .by_ref()
            .take(100)
            .map(|(_, node)| crate::FileDeletionSummary {
                file_id: node.file_id.clone(),
                deletion_node_id: node.node_id.clone(),
                repository_id: node.repository_id.clone(),
                path: node.path.as_str().into(),
            })
            .collect::<Vec<_>>();
        let next_after = if nodes.next().is_some() {
            Some(
                entries
                    .last()
                    .expect("invariant: another record follows a full page")
                    .file_id
                    .clone(),
            )
        } else {
            None
        };
        Ok(crate::FileDeletionsPage {
            snapshot: engine.listing_snapshot().into(),
            entries,
            next_after,
        })
    }

    pub(super) fn file_deletion_batch_undo_preview_admitted(
        &self,
        identity: &Identity,
        operation: storage_contract::OperationId,
    ) -> Result<storage_contract::file_deletion_batch::FileDeletionBatchUndoPreview, ApiError> {
        let (engine, current) = self.lock_current(identity)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        let actor = storage_engine::BatchUndoActor::parse(&current.principal)
            .map_err(storage_engine::Error::BatchUndo)
            .map_err(error)?;
        engine
            .preflight_file_deletion_batch_undo(operation, actor)
            .map_err(error)
    }

    pub(super) fn restore_deleted_file_batch_admitted(
        &self,
        identity: &Identity,
        request: &storage_contract::file_deletion_batch::UndoFileDeletionBatchBody,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        let actor = storage_engine::BatchUndoActor::parse(&current.principal)
            .map_err(storage_engine::Error::BatchUndo)
            .map_err(error)?;
        engine
            .undo_file_deletion_batch(storage_engine::UndoFileDeletionBatchRequest {
                deletion_operation_id: request.deletion_operation_id,
                principal: actor,
                operation_id: request.operation_id,
            })
            .map_err(error)
    }

    pub(super) fn restore_deleted_file_admitted(
        &self,
        identity: &Identity,
        command: crate::RestoreDeletedFile,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        engine
            .undo_file_deletion(storage_engine::UndoFileDeletionRequest {
                file_id: &command.file_id,
                deletion_node_id: &command.deletion_node_id,
                principal: &current.principal,
                operation_id: command.operation_id,
            })
            .map_err(error)
    }
}
