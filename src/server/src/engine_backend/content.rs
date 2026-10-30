//! Authorized content read plans and their unlocked I/O execution.
use super::{
    EngineBackend, current_protected_identity, error, repo_for_history, repository,
    workspace_authorize,
};
use crate::{ApiError, Content, auth::Identity};

impl EngineBackend {
    pub(super) fn open_version_content(
        &self,
        identity: &Identity,
        request: crate::ReadContent<'_>,
    ) -> Result<Content, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let version = request.version;
        // Context checks and the content plan share this admission. The name is
        // a request-local snapshot, not permission to resolve the path later.
        let (record, logical_name) = if let Some(context) = request.context {
            let repo = repository(&engine, &context.repository_id)?;
            let record = engine.state().versions.get(version).ok_or_else(|| {
                ApiError::new(
                    ::http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::VersionNotFound,
                    "version missing",
                )
            })?;
            if record.history_tree_id != repo.history_tree_id {
                return Err(ApiError::new(
                    ::http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::FileNotFound,
                    "version is not in this repository context",
                ));
            }
            let entry = engine
                .namespace_tree(&repo.main_branch_id)
                .map_err(error)?
                .get_key(context.path.key())
                .filter(|entry| {
                    entry.kind == storage_contract::NamespaceEntryKind::File
                        && entry.file_id == record.file_id
                })
                .ok_or_else(|| {
                    ApiError::new(
                        ::http::StatusCode::NOT_FOUND,
                        storage_contract::ErrorCode::FileNotFound,
                        "version file is not present at this logical path",
                    )
                })?;
            (record, Some(entry.name().to_owned()))
        } else {
            let record = engine.state().versions.get(version).ok_or_else(|| {
                ApiError::new(
                    ::http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::NotFound,
                    "version not found",
                )
            })?;
            (record, None)
        };
        let repo = repo_for_history(&engine, &record.history_tree_id)?;
        let plan = engine.prepare_content(version).map_err(error)?;
        let proof_memory =
            crate::content_proof_budget::reserve(plan.checksum_manifest_bytes().map_err(error)?)?;
        let download_name = match logical_name {
            Some(name) => name,
            None => default_download_name(&engine, repo, record)?,
        };
        drop(engine);
        let content = plan.open().map_err(error)?;
        Ok(Content {
            checksum_manifest: Some(content.manifest),
            fallback_files: content.fallback_files,
            file: content.file,
            size: content.size,
            hash: content.content_hash.to_string(),
            lease: Box::new((content.lease, proof_memory)),
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
            hash: opened.content_hash.to_string(),
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

fn default_download_name(
    engine: &storage_engine::Engine,
    repo: &storage_contract::Repository,
    record: &storage_contract::FileVersion,
) -> Result<String, ApiError> {
    let tree = engine.namespace_tree(&repo.main_branch_id).map_err(error)?;
    let entries = tree.entries();
    let preferred = engine
        .state()
        .file_locations
        .get(&record.file_id)
        .and_then(|path| {
            tree.get(path).filter(|entry| {
                entry.kind == storage_contract::NamespaceEntryKind::File
                    && entry.file_id == record.file_id
            })
        });
    Ok(preferred
        .or_else(|| {
            entries.iter().find(|entry| {
                entry.kind == storage_contract::NamespaceEntryKind::File
                    && entry.file_id == record.file_id
            })
        })
        .map(|entry| entry.name().to_owned())
        .or_else(|| engine.state().file_names.get(&record.file_id).cloned())
        .unwrap_or_else(|| record.creation_name.clone()))
}
