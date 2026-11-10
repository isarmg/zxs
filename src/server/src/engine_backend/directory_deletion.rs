//! Captured SMB directory deletion and management-only restoration share one
//! protected engine owner. SMB cannot borrow the admin restoration endpoint.
use super::*;
use storage_contract::directory_git::{DirectoryDeletionNode, UndoDirectoryDeletionBody};

impl EngineBackend {
    pub(super) fn current_directory_deletion_admitted(
        &self,
        identity: &Identity,
        repository: &str,
        owner: &str,
        directory: &str,
    ) -> Result<Option<DirectoryDeletionNode>, ApiError> {
        let (engine, current) = self.lock_current(identity)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        Ok(engine
            .directory_deletion(repository, owner, directory)
            .cloned())
    }

    pub(super) fn restore_deleted_directory_admitted(
        &self,
        identity: &Identity,
        repository: &str,
        request: &UndoDirectoryDeletionBody,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        // The exact original operation is replayed before reading today's D.
        engine
            .undo_directory_deletion(repository, request, &current.principal)
            .map_err(error)
    }
}

fn bound_repository(engine: &Engine, current: &Identity, branch: &str) -> Result<String, ApiError> {
    if current
        .smb_binding
        .as_ref()
        .is_some_and(|binding| binding.branch_id != branch)
    {
        return Err(ApiError::forbidden());
    }
    let repository = repo_for_branch(engine, branch)?;
    if !repository.file_manager_root || repository.main_branch_id != branch {
        return Err(ApiError::unsupported());
    }
    Ok(repository.repository_id.clone())
}

fn require_empty(
    engine: &Engine,
    branch: &str,
    path: &name_policy::LookupPath,
    expected_entry: &str,
) -> Result<(), ApiError> {
    let object = engine
        .locate_object(branch, path)
        .map_err(error)?
        .ok_or_else(|| {
            ApiError::new(
                http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::DirectoryNotFound,
                "directory is no longer visible",
            )
        })?;
    let entry = object.entry();
    if entry.kind != storage_contract::NamespaceEntryKind::Directory {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::NotDirectory,
            "rmdir requires a directory",
        ));
    }
    if entry.entry_id != expected_entry {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::WorkspaceChanged,
            "directory identity changed",
        ));
    }
    if object.has_visible_children() {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::DirectoryNotEmpty,
            "SMB directory removal requires an empty visible directory",
        ));
    }
    Ok(())
}

impl EngineBackend {
    pub(super) fn prepare_current_directory_deletion_admitted(
        &self,
        identity: &Identity,
        command: crate::PrepareCurrentDirectoryDeletion,
    ) -> Result<Option<crate::CurrentDirectoryDeletion>, ApiError> {
        let (engine, current) = self.lock_current(identity)?;
        let repository = bound_repository(&engine, &current, &command.branch_id)?;
        require_empty(
            &engine,
            &command.branch_id,
            &command.path,
            &command.expected_entry_id,
        )?;
        engine
            .prepare_current_directory_deletion(
                &repository,
                &command.path,
                &command.expected_entry_id,
            )
            .map(|request| {
                request.map(|request| crate::CurrentDirectoryDeletion {
                    repository_id: repository,
                    branch_id: command.branch_id,
                    request,
                })
            })
            .map_err(error)
    }

    pub(super) fn delete_current_directory_admitted(
        &self,
        identity: &Identity,
        command: crate::DeleteCurrentDirectory,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        let captured = command.captured;
        let repository = bound_repository(&engine, &current, &captured.branch_id)?;
        if repository != captured.repository_id
            || captured.request.kind != storage_contract::DirectoryBatchKind::Delete
            || captured.request.items.len() != 1
            || captured.request.items[0].target.is_some()
        {
            return Err(ApiError::invalid("directory deletion capture differs"));
        }
        if !engine
            .state()
            .operations
            .contains_key(&command.operation_id)
        {
            let source = &captured.request.items[0].source;
            let path =
                name_policy::LookupPath::parse(&source.path).map_err(ApiError::invalid_source)?;
            require_empty(&engine, &captured.branch_id, &path, &source.entry_id)?;
        }
        // The engine admits the original canonical request before today's
        // visibility. No fallback, fresh path capture or second operation.
        engine
            .directory_namespace_batch(
                &repository,
                &captured.request,
                &current.principal,
                command.operation_id,
            )
            .map_err(error)
    }
}

impl EngineBackend {
    pub(super) fn directory_deletions_admitted(
        &self,
        identity: &Identity,
        repository: &str,
        owner: &str,
        request: crate::ReadDirectoryDeletions,
    ) -> Result<storage_contract::directory_git::DirectoryDeletionsPage, ApiError> {
        use storage_contract::directory_git::{DirectoryDeletionSummary, DirectoryDeletionsPage};
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
                "directory deletions changed; restart listing",
            ));
        }
        let git = engine
            .state()
            .directory_git_repositories
            .get(owner)
            .filter(|git| git.repository_id == repository)
            .ok_or_else(|| {
                ApiError::new(
                    http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::DirectoryNotFound,
                    "initialized directory owner not found",
                )
            })?;
        let root = &engine.state().repositories[repository];
        let namespace = engine.list_tree_ref(&root.main_branch_id).map_err(error)?;
        // D keeps its immutable source path. Current listings use the stable
        // directory identity's admitted mount, including hidden raw rows.
        let locations = namespace
            .iter()
            .filter(|entry| {
                entry.kind == storage_contract::NamespaceEntryKind::Directory
                    && git.deletions.contains_key(&entry.entry_id)
            })
            .map(|entry| (entry.entry_id.as_str(), entry))
            .collect::<std::collections::BTreeMap<_, _>>();
        let lower = request
            .after
            .as_deref()
            .map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded);
        let mut nodes = git
            .deletions
            .range::<str, _>((lower, std::ops::Bound::Unbounded))
            .filter_map(|(id, node)| locations.get(id.as_str()).map(|entry| (node, *entry)));
        let entries = nodes
            .by_ref()
            .take(100)
            .map(|(node, entry)| DirectoryDeletionSummary {
                directory_id: node.directory_id.clone(),
                deletion_node_id: node.node_id.clone(),
                path: entry.path.clone(),
                operation_id: node.operation_id,
            })
            .collect::<Vec<_>>();
        let next_after = if nodes.next().is_some() {
            Some(
                entries
                    .last()
                    .expect("invariant: another record follows a full page")
                    .directory_id
                    .clone(),
            )
        } else {
            None
        };
        Ok(DirectoryDeletionsPage {
            snapshot: engine.listing_snapshot().into(),
            entries,
            next_after,
        })
    }
}
