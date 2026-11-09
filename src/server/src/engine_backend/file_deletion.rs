//! The same core publication owns SMB deletion and management restoration.
use super::*;
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

impl EngineBackend {
    pub(super) fn soft_delete_file_admitted(
        &self,
        identity: &Identity,
        command: crate::DeleteFile,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        // Bound a new HTTP publication to the same complete-frontier budget as
        // discovery. The core still captures the authoritative frontier when it
        // plans the mutation; both captures occur under this owner lock.
        if !engine
            .state()
            .operations
            .contains_key(&command.operation_id)
        {
            let capture = engine
                .file_deletion_preflight(
                    storage_engine::FileDeletionTarget {
                        repository_id: &command.repository_id,
                        file_id: &command.file_id,
                        path: &command.path,
                    },
                    &storage_contract::history_window::HistoryWindowCancellation::default(),
                )
                .map_err(error)?;
            if capture.branches().len() > 4096 || capture.parent_node_ids().len() > 4096 {
                return Err(ApiError::limit(
                    "complete deletion frontier exceeds response record budget",
                ));
            }
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
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        if current
            .smb_binding
            .as_ref()
            .is_some_and(|binding| binding.branch_id != command.branch_id)
        {
            return Err(ApiError::forbidden());
        }
        let repository = repo_for_branch(&engine, &command.branch_id)?;
        if !repository.file_manager_root || repository.main_branch_id != command.branch_id {
            return Err(ApiError::unsupported());
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
            return engine.soft_delete_file(request).map_err(error);
        }
        let namespace = engine.namespace_tree(&command.branch_id).map_err(error)?;
        let entry = namespace.get_key(command.path.key()).ok_or_else(|| {
            ApiError::new(
                http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::FileNotFound,
                "delete target is no longer visible",
            )
        })?;
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
        engine.soft_delete_file(request).map_err(error)
    }

    pub(super) fn current_file_deletion_admitted(
        &self,
        identity: &Identity,
        file: &HistoryFileId,
    ) -> Result<Option<storage_contract::FileDeletionNode>, ApiError> {
        let engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        let node = engine.file_deletion(file.as_str());
        if node
            .is_some_and(|node| node.branch_heads.len() > 4096 || node.parent_node_ids.len() > 4096)
        {
            return Err(ApiError::limit(
                "complete deletion frontier exceeds response record budget",
            ));
        }
        Ok(node.cloned())
    }

    pub(super) fn file_deletions_admitted(
        &self,
        identity: &Identity,
        request: crate::ReadFileDeletions,
    ) -> Result<crate::FileDeletionsPage, ApiError> {
        let engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
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

    pub(super) fn restore_deleted_file_admitted(
        &self,
        identity: &Identity,
        command: crate::RestoreDeletedFile,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
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
