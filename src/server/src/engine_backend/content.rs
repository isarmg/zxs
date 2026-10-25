//! Authorized content read plans and their unlocked I/O execution.
use super::{
    EngineBackend, current_protected_identity, error, repo_for_history, workspace_authorize,
};
use crate::{ApiError, Content, auth::Identity};

impl EngineBackend {
    pub(super) fn open_version_content(
        &self,
        identity: &Identity,
        version: &str,
    ) -> Result<Content, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let record = engine.state().versions.get(version).ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "version not found",
            )
        })?;
        repo_for_history(&engine, &record.history_tree_id)?;
        let plan = engine.prepare_content(version).map_err(error)?;
        let repo = repo_for_history(&engine, &record.history_tree_id)?;
        let tree = engine.namespace_tree(&repo.main_branch_id).ok();
        let entries = tree.map(|tree| tree.entries()).unwrap_or_default();
        let preferred = engine
            .state()
            .file_locations
            .get(&record.file_id)
            .and_then(|path| {
                tree?
                    .get(path)
                    .filter(|entry| entry.kind == "file" && entry.file_id == record.file_id)
            });
        let download_name = preferred
            .or_else(|| {
                entries
                    .iter()
                    .find(|entry| entry.kind == "file" && entry.file_id == record.file_id)
            })
            .map(|entry| entry.name.clone())
            .or_else(|| engine.state().file_names.get(&record.file_id).cloned())
            .unwrap_or_else(|| record.creation_name.clone());
        drop(engine);
        let content = plan.open().map_err(error)?;
        Ok(Content {
            checksum_manifest: Some(content.manifest),
            fallback_files: content.fallback_files,
            file: content.file,
            size: content.size,
            hash: content.content_hash,
            lease: Box::new(content.lease),
            download_name: Some(download_name),
        })
    }
    pub(super) fn open_workspace_content(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
    ) -> Result<Content, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let workspace_record = workspace_authorize(&engine, &current, workspace)?;
        let plan = engine
            .prepare_workspace_content(workspace, item, &workspace_record.principal)
            .map_err(error)?;
        drop(engine);
        let opened = plan.open().map_err(error)?;
        Ok(Content {
            checksum_manifest: None,
            fallback_files: Vec::new(),
            file: opened.file,
            size: opened.size,
            hash: opened.content_hash,
            lease: Box::new(opened.lease),
            download_name: None,
        })
    }
    pub(super) fn read_version_checksums(
        &self,
        identity: &Identity,
        version: &str,
    ) -> Result<Vec<u8>, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let record = engine.state().versions.get(version).ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "version not found",
            )
        })?;
        repo_for_history(&engine, &record.history_tree_id)?;
        let plan = engine.prepare_content(version).map_err(error)?;
        drop(engine);
        plan.checksums().map_err(error)
    }
}
