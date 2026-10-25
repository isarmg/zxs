//! Dirty generations, recovery records and publication acknowledgement.
use super::*;

pub(super) fn recovery_record(
    writer: &Writer,
    principal: &str,
    branch: &str,
) -> Result<(), ApiError> {
    write_recovery_record(writer, principal, branch, None)
}

fn write_recovery_record(
    writer: &Writer,
    principal: &str,
    branch: &str,
    detached: Option<(&ApiError, bool)>,
) -> Result<(), ApiError> {
    writer
        .directory
        .verify_identity()
        .map_err(crate::upload_fs_error)?;
    // `published` describes the last reconciled operation, not all bytes
    // currently reachable through the live writable descriptor. Recovery must
    // reconcile these bytes even when no Dirty request was received.
    let mut value = json!({
        "format": "smb-protocol-work",
        "principal": principal,
        "branch_id": branch,
        "path": writer.path,
        "item_id": writer.item_id,
        "source_path": writer.source,
        "workspace_id": writer.workspace.workspace_id,
        "expected_head": writer.workspace.base_head,
        "fencing_token": writer.workspace.fencing_token.to_string(),
        "operation_id": writer.operation_id,
        "dirty": writer.dirty,
        "published": writer.committed,
        "descriptor_changes_unconfirmed": true,
        "handle_state": if detached.is_some() { "detached" } else { "active" }
    });
    if let Some((error, synced)) = detached {
        value["detach_error"] = json!({"code": error.code, "message": error.message});
        value["retained_bytes_synced"] = json!(synced);
    }
    use platform_io::fs_safety::{AtomicFile, RelativePath};
    let record = writer.source.with_extension("work.json");
    let name = RelativePath::new(Path::new(
        record
            .file_name()
            .ok_or_else(|| ApiError::invalid("SMB record filename missing"))?,
    ))
    .map_err(crate::upload_fs_error)?;
    AtomicFile::replace(
        &writer.directory,
        &name,
        &json_codec::to_vec(&value).map_err(ApiError::invalid)?,
    )
    .map_err(crate::upload_fs_error)
}

fn retain_detached(writer: &Writer, principal: &str, branch: &str, reason: &ApiError) {
    // A Close error must not leave an unreachable active writer. Try to make
    // its latest bytes and detached state durable, but even on an I/O failure
    // keep the pre-existing source/record and release the live reservation.
    let synced = match writer.file.sync_all() {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(
                %error, operation_id = %writer.operation_id,
                source = %writer.source.display(), "detached SMB bytes could not be synced"
            );
            false
        }
    };
    if let Err(error) = write_recovery_record(writer, principal, branch, Some((reason, synced))) {
        tracing::warn!(
            %error, operation_id = %writer.operation_id,
            source = %writer.source.display(), "detached SMB recovery record could not be refreshed"
        );
    }
}

pub(super) fn cleanup_writer(writer: &Writer) {
    for path in [&writer.source, &writer.source.with_extension("work.json")] {
        if let Err(error) = fs::remove_file(path)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(%error,path=%path.display(),"SMB staging cleanup failed");
        }
    }
    writer.reservation.cleanup_record();
}

pub(super) fn finish_publication(
    shared: &Shared,
    identity: &Identity,
    branch: &str,
    writer: &mut Writer,
    result: Value,
) -> Result<Value, ApiError> {
    writer.last_head = field(result.get("result").unwrap_or(&result), "commit_id")?.to_owned();
    // Bytes may have changed after a response was lost. Never acknowledge the
    // earlier publication as if it included those later FD-backed writes.
    let tree = shared.backend.call(
        crate::BackendAction::Tree,
        identity,
        json!({"branch_id":branch,"query":{"commit_id":writer.last_head}}),
    )?;
    let entries: Vec<Entry> =
        json_codec::from_value(tree["entries"].clone()).map_err(ApiError::invalid)?;
    let entry = entries
        .iter()
        .find(|entry| entry.entry_id == writer.item_id)
        .ok_or_else(|| {
            ApiError::conflict(
                storage_contract::ErrorCode::FileVersionConflict,
                "published file identity missing",
            )
        })?;
    let published = shared.backend.content(identity, &entry.version_id)?;
    let mut hash = blake3::Hasher::new();
    let mut offset = 0u64;
    let mut buffer = [0u8; 65_536];
    loop {
        let length = writer
            .file
            .read_at(&mut buffer, offset)
            .map_err(ApiError::io)?;
        if length == 0 {
            break;
        }
        hash.update(&buffer[..length]);
        offset = offset
            .checked_add(length as u64)
            .ok_or_else(|| ApiError::limit("file byte count overflow"))?;
    }
    writer.dirty = offset != published.size || hash.finalize().to_hex().as_str() != published.hash;
    // A failed comparison must remain reconcilable by the original operation.
    // Only a completed comparison may require a new authorized dirty generation.
    writer.committed = true;
    if let Err(error) = recovery_record(writer, &identity.principal, branch) {
        tracing::warn!(%error,"published SMB recovery record could not be refreshed");
    }
    if writer.dirty {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::WorkspaceRebaseRequired,
            "original save published; later changed bytes are retained and need a new dirty request",
        ));
    }
    Ok(result)
}

impl Session {
    pub(super) fn dirty(&mut self, handle: u64) -> Result<Value, ApiError> {
        // The FD has already been modified. Preserve it even when current
        // authorization or rebase validation rejects the operation.
        let (needs_workspace, path, last_head, captured_workspace) =
            match self.handles.get_mut(&handle) {
                Some(Handle::Write(writer)) => {
                    writer.dirty = true;
                    recovery_record(writer, &self.identity.principal, &self.branch)?;
                    (
                        writer.committed,
                        writer.path.clone(),
                        writer.last_head.clone(),
                        writer.workspace.workspace_id.clone(),
                    )
                }
                _ => return Err(ApiError::forbidden()),
            };
        self.call(crate::BackendAction::Tree, json!({"branch_id":self.branch}))?;
        // Even an already published handle remains bound to the selection it
        // opened. A same-content default switch must not retarget this FD.
        self.call(
            crate::BackendAction::WorkspaceSelectionAuthorize,
            json!({
                "workspace_id":captured_workspace,"path":path
            }),
        )?;
        let new_workspace = if needs_workspace {
            let workspace = self.workspace()?;
            let validate = (|| {
                let original = self.call(
                    crate::BackendAction::Tree,
                    json!({"branch_id":self.branch,"query":{"commit_id":last_head}}),
                )?;
                let current = self.call(
                    crate::BackendAction::Tree,
                    json!({"branch_id":self.branch,"query":{"commit_id":workspace.base_head}}),
                )?;
                let select = |tree: &Value| -> Result<Option<Entry>, ApiError> {
                    let entries: Vec<Entry> = json_codec::from_value(tree["entries"].clone())
                        .map_err(ApiError::invalid)?;
                    Ok(entries
                        .into_iter()
                        .find(|entry| entry.lookup_key == name_policy::path_key(&path)))
                };
                let previous = select(&original)?;
                let current = select(&current)?;
                if previous
                    .as_ref()
                    .map(|e| (&e.entry_id, &e.version_id, &e.path))
                    != current
                        .as_ref()
                        .map(|e| (&e.entry_id, &e.version_id, &e.path))
                {
                    return Err(ApiError::conflict(
                        storage_contract::ErrorCode::FileVersionConflict,
                        "the open file changed since this handle last published",
                    ));
                }
                Ok(())
            })();
            if let Err(error) = validate {
                let _ = self.call(
                    crate::BackendAction::AbortWorkspace,
                    json!({"workspace_id":workspace.workspace_id}),
                );
                return Err(error);
            }
            Some(workspace)
        } else {
            if let Some(Handle::Write(writer)) = self.handles.get(&handle) {
                self.call(
                    crate::BackendAction::UploadAuthorize,
                    json!({"workspace_id":writer.workspace.workspace_id,"item_id":writer.item_id}),
                )?;
            }
            None
        };
        let Some(Handle::Write(writer)) = self.handles.get_mut(&handle) else {
            return Err(ApiError::forbidden());
        };
        if let Some(workspace) = new_workspace {
            writer.workspace = workspace;
            writer.committed = false;
            writer.operation_id = new_id("op");
        }
        recovery_record(writer, &self.identity.principal, &self.branch)?;
        if writer.file.metadata().map_err(ApiError::io)?.len() > self.max_file_bytes {
            return Err(ApiError::limit("file exceeds SMB byte limit"));
        }
        if writer.write_through {
            self.flush(handle)
        } else {
            Ok(json!({"dirty":true}))
        }
    }
    pub(super) fn flush(&mut self, handle: u64) -> Result<Value, ApiError> {
        let Some(entry) = self.handles.get_mut(&handle) else {
            return Err(ApiError::new(
                404,
                storage_contract::ErrorCode::NotFound,
                "SMB handle missing",
            ));
        };
        let writer = match entry {
            Handle::Read(content, _) => {
                content.file.sync_all().map_err(ApiError::io)?;
                return Ok(json!({"saved":true,"changed":false}));
            }
            Handle::Directory(directory) => {
                directory.file.sync_all().map_err(ApiError::io)?;
                return Ok(json!({"saved":true,"changed":false}));
            }
            Handle::Write(writer) => writer,
        };
        if !writer.dirty {
            return Ok(json!({"saved":true,"changed":false}));
        }
        if writer.committed {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::WorkspaceRebaseRequired,
                "changed bytes need an authorized workspace before saving",
            ));
        }
        // A lost response must not restage under a new dirty generation and
        // thereby change the original request digest. Reconcile by operation id.
        match self.shared.backend.call(
            crate::BackendAction::Operation,
            &self.identity,
            json!({"operation_id":writer.operation_id}),
        ) {
            Ok(result)
                if result.get("commit_state").and_then(Value::as_str) == Some("PUBLISHED") =>
            {
                return finish_publication(
                    &self.shared,
                    &self.identity,
                    &self.branch,
                    writer,
                    result,
                );
            }
            Ok(_) => {
                return Err(ApiError::new(
                    503,
                    storage_contract::ErrorCode::RecoveryRequired,
                    "SMB save outcome must be reconciled before retrying",
                ));
            }
            Err(error)
                if error.code == storage_contract::ErrorCode::NotFound
                    || error.code == storage_contract::ErrorCode::OperationNotFound => {}
            Err(error) => return Err(error),
        }
        writer
            .directory
            .verify_payload(
                &platform_io::fs_safety::EntryName::new(
                    writer
                        .source
                        .file_name()
                        .ok_or_else(|| ApiError::invalid("SMB source filename missing"))?,
                )
                .map_err(crate::upload_fs_error)?,
                &writer.file,
            )
            .map_err(crate::upload_fs_error)?;
        writer.file.sync_all().map_err(ApiError::io)?;
        if writer.file.metadata().map_err(ApiError::io)?.len() > self.max_file_bytes {
            return Err(ApiError::limit("file exceeds SMB byte limit"));
        }
        self.shared.backend.upload(
            &self.identity,
            &writer.workspace.workspace_id,
            &writer.item_id,
            &writer.path,
            &crate::UploadSource::from_file(
                writer.file.try_clone().map_err(ApiError::io)?,
                self.max_file_bytes,
            )?,
        )?;
        let result = self.shared.backend.call(
            crate::BackendAction::CommitWorkspaceInternal,
            &self.identity,
            json!({
                "workspace_id": writer.workspace.workspace_id,
                "operation_id": writer.operation_id,
                "expected_head": writer.workspace.base_head,
                "fencing_token": writer.workspace.fencing_token.to_string(),
                "message": "SMB full-file save"
            }),
        )?;
        finish_publication(&self.shared, &self.identity, &self.branch, writer, result)
    }

    /// Unlike Flush, Close always consumes the handle. A failed publication
    /// becomes retained detached work, not a lock with no VFS owner.
    pub(super) fn close(&mut self, handle: u64, abort: bool) -> Result<Value, ApiError> {
        if !self.handles.contains_key(&handle) {
            return Err(ApiError::new(
                404,
                storage_contract::ErrorCode::NotFound,
                "SMB handle missing",
            ));
        }
        let saved = (|| {
            if !abort {
                self.flush(handle)?;
            }
            if let Some(Handle::Write(writer)) = self.handles.get(&handle)
                && !writer.committed
            {
                self.call(
                    crate::BackendAction::AbortWorkspace,
                    json!({"workspace_id":writer.workspace.workspace_id}),
                )?;
            }
            Ok(())
        })();
        if let Some(Handle::Write(writer)) = self.handles.remove(&handle) {
            match &saved {
                Ok(()) => cleanup_writer(&writer),
                Err(error) => {
                    retain_detached(&writer, &self.identity.principal, &self.branch, error);
                }
            }
            self.release_paths(&[name_policy::path_key(&writer.path)]);
        }
        saved?;
        Ok(json!({"closed":true,"aborted":abort}))
    }

    /// Used when a local worker failed before Close could safely attempt a save.
    /// Detach is not Abort: no backend mutation, source deletion or new operation.
    pub(super) fn detach(&mut self, handle: u64) -> Result<Value, ApiError> {
        let entry = self.handles.remove(&handle).ok_or_else(|| {
            ApiError::new(
                404,
                storage_contract::ErrorCode::NotFound,
                "SMB handle missing",
            )
        })?;
        let mut result = json!({"closed":true,"detached":true});
        if let Handle::Write(writer) = entry {
            let reason = ApiError::new(
                503,
                storage_contract::ErrorCode::RecoveryRequired,
                "SMB handle detached without saving; retained work requires reconciliation",
            );
            retain_detached(&writer, &self.identity.principal, &self.branch, &reason);
            self.release_paths(&[name_policy::path_key(&writer.path)]);
            result["operation_id"] = json!(writer.operation_id);
        }
        Ok(result)
    }
}
