//! Dirty generations, recovery records and publication acknowledgement.
use super::*;

impl Writer {
    /// One canonical request for this save generation, including reconciliation.
    /// A successor workspace and operation are installed together by Dirty.
    fn save_request(&self) -> crate::CommitWorkspace {
        crate::CommitWorkspace {
            workspace_id: self.workspace.workspace_id.clone(),
            operation_id: self.operation_id,
            expected_head: self.workspace.base_head.clone(),
            fencing_token: self.workspace.fencing_token,
            message: "SMB full-file save".into(),
            request_digest: None,
        }
    }
}

pub(super) fn recovery_record(
    writer: &mut Writer,
    principal: &str,
    branch: &str,
) -> Result<(), ApiError> {
    write_recovery_record(writer, principal, branch, None)
}

fn write_recovery_record(
    writer: &mut Writer,
    principal: &str,
    branch: &str,
    detached: Option<(&ApiError, bool)>,
) -> Result<(), ApiError> {
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
    use platform_io::fs_safety::{AtomicFile, EntryName};
    let record = writer.source.with_extension("work.json");
    let name = EntryName::new(
        record
            .file_name()
            .ok_or_else(|| ApiError::invalid("SMB record filename missing"))?,
    )
    .map_err(crate::upload_fs_error)?;
    let bytes =
        json_codec::to_vec(&value).expect("invariant: internal JSON serialization succeeds");
    if detached.is_some() {
        writer
            .directory
            .verify_identity()
            .map_err(crate::upload_fs_error)?;
        AtomicFile::replace(&writer.directory, &name.as_relative(), &bytes)
            .map_err(crate::upload_fs_error)
    } else {
        writer
            .recovery_record
            .persist(&writer.directory, &name, bytes)
    }
}

fn retain_detached(writer: &mut Writer, principal: &str, branch: &str, reason: &ApiError) {
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
            tracing::warn!(error = ?error,path=%path.display(),"SMB staging cleanup failed");
        }
    }
}

pub(super) fn finish_publication(
    shared: &Shared,
    identity: &Identity,
    branch: &str,
    writer: &mut Writer,
    result: storage_contract::OperationOutcome,
) -> Result<Value, ApiError> {
    shared
        .backend
        .require_native_workspace_commit_completion(identity, writer.save_request())?;
    writer.last_head = result
        .commit_id
        .clone()
        .expect("invariant: published workspace operation identifies its commit");
    // Bytes may have changed after a response was lost. Never acknowledge the
    // earlier publication as if it included those later FD-backed writes.
    let snapshot = shared
        .backend
        .snapshot_at(identity, branch, &writer.last_head)?;
    let entry = snapshot
        .entries
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
        tracing::warn!(error = ?error,"published SMB recovery record could not be refreshed");
    }
    if writer.dirty {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::WorkspaceRebaseRequired,
            "original save published; later changed bytes are retained and need a new dirty request",
        ));
    }
    crate::operation_wire::operation_wire(result)
}

impl Session {
    /// Admit one descriptor mutation before the VFS performs it. This is the
    /// per-mutation authorization point: the peer announces only the first
    /// change of a generation with Dirty (every change when write-through).
    pub(super) fn prepare_write(
        &mut self,
        handle: u64,
        end: u64,
        bytes: u64,
    ) -> Result<Value, ApiError> {
        let Some(Handle::Write(writer, _)) = self.handles.get_mut(&handle) else {
            return Err(ApiError::forbidden());
        };
        self.shared
            .backend
            .authorize_current(&self.identity, &self.branch)?;
        self.shared.backend.authorize_workspace_selection(
            &self.identity,
            &writer.workspace.workspace_id,
            &writer.path,
        )?;
        // A published generation has no open workspace to write into; the
        // following Dirty creates and authorizes its successor.
        if !writer.committed {
            self.shared.backend.authorize_upload(
                &self.identity,
                &writer.workspace.workspace_id,
                None,
            )?;
        }
        let current = writer.file.metadata().map_err(ApiError::io)?.len();
        writer
            .reservation
            .before_write(end, bytes.max(end.saturating_sub(current)))?;
        Ok(json!({"ready":true}))
    }

    pub(super) fn dirty(&mut self, handle: u64) -> Result<Value, ApiError> {
        // The FD has already been modified. Preserve it even when current
        // authorization or rebase validation rejects the operation.
        let Some(Handle::Write(writer, _)) = self.handles.get_mut(&handle) else {
            return Err(ApiError::forbidden());
        };
        writer.dirty = true;
        recovery_record(writer, &self.identity.principal, &self.branch)?;
        self.shared
            .backend
            .authorize_current(&self.identity, &self.branch)?;
        // Even an already published handle remains bound to the selection it
        // opened. A same-content default switch must not retarget this FD.
        self.shared.backend.authorize_workspace_selection(
            &self.identity,
            &writer.workspace.workspace_id,
            &writer.path,
        )?;
        if writer.committed {
            let operation_id = OperationId::generate().map_err(ApiError::contract_input)?;
            let workspace = self.shared.workspace(&self.identity, &self.branch)?;
            let validate = (|| {
                let lookup = name_policy::LookupPath::from(&writer.path);
                let previous = self.shared.backend.lookup_at(
                    &self.identity,
                    &self.branch,
                    &writer.last_head,
                    &lookup,
                )?;
                let current = self.shared.backend.lookup_at(
                    &self.identity,
                    &self.branch,
                    &workspace.base_head,
                    &lookup,
                )?;
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
                if let Err(abort_error) = self
                    .shared
                    .backend
                    .abort_workspace(&self.identity, &workspace.workspace_id)
                {
                    tracing::warn!(error = ?abort_error, "SMB preparation abort failed; workspace retained for recovery");
                }
                return Err(error);
            }
            writer.workspace = workspace;
            writer.committed = false;
            writer.operation_id = operation_id;
            recovery_record(writer, &self.identity.principal, &self.branch)?;
        } else {
            self.shared.backend.authorize_upload(
                &self.identity,
                &writer.workspace.workspace_id,
                None,
            )?;
        }
        if writer.file.metadata().map_err(ApiError::io)?.len() > self.max_file_bytes {
            return Err(ApiError::limit("file exceeds SMB byte limit"));
        }
        if writer.write_through {
            flush_writer(
                &self.shared,
                &self.identity,
                &self.branch,
                self.max_file_bytes,
                writer,
            )
        } else {
            Ok(json!({"dirty":true}))
        }
    }
    pub(super) fn flush(&mut self, handle: u64) -> Result<Value, ApiError> {
        let opened = self
            .handles
            .get_mut(&handle)
            .ok_or_else(handles::missing_handle)?;
        match opened {
            Handle::Write(writer, _) if writer.dirty => flush_writer(
                &self.shared,
                &self.identity,
                &self.branch,
                self.max_file_bytes,
                writer,
            ),
            opened => {
                // No-change acknowledgements still require current authority.
                opened.authorize_read(&self.shared, &self.identity, &self.branch)?;
                match opened {
                    Handle::Read(content, _) => content.file.sync_all().map_err(ApiError::io)?,
                    Handle::Directory(directory) => {
                        directory.file.sync_all().map_err(ApiError::io)?;
                    }
                    Handle::Write(_, _) => {}
                }
                Ok(json!({"saved":true,"changed":false}))
            }
        }
    }

    /// Unlike Flush, Close always consumes the handle. A failed publication
    /// becomes retained detached work, not a lock with no VFS owner.
    pub(super) fn close(&mut self, handle: u64, abort: bool) -> Result<Value, ApiError> {
        if self
            .read_cache
            .as_ref()
            .is_some_and(|(owner, _)| *owner == handle)
        {
            self.read_cache = None;
        }
        let opened = self
            .handles
            .remove(&handle)
            .ok_or_else(handles::missing_handle)?;
        if let Handle::Write(mut writer, _paths) = opened {
            // Close owns the descriptor even when publication fails. Its path
            // reservation stays held until cleanup or detached retention finishes.
            let saved = (|| {
                if !abort && writer.dirty {
                    flush_writer(
                        &self.shared,
                        &self.identity,
                        &self.branch,
                        self.max_file_bytes,
                        &mut writer,
                    )?;
                }
                if !writer.committed {
                    self.shared
                        .backend
                        .abort_workspace(&self.identity, &writer.workspace.workspace_id)?;
                }
                Ok(())
            })();
            match &saved {
                Ok(()) => cleanup_writer(&writer),
                Err(error) => {
                    retain_detached(&mut writer, &self.identity.principal, &self.branch, error);
                }
            }
            saved?;
        }
        Ok(json!({"closed":true,"aborted":abort}))
    }

    /// Used when a local worker failed before Close could safely attempt a save.
    /// Detach is not Abort: no backend mutation, source deletion or new operation.
    pub(super) fn detach(&mut self, handle: u64) -> Result<Value, ApiError> {
        if self
            .read_cache
            .as_ref()
            .is_some_and(|(owner, _)| *owner == handle)
        {
            self.read_cache = None;
        }
        let entry = self
            .handles
            .remove(&handle)
            .ok_or_else(handles::missing_handle)?;
        let mut result = json!({"closed":true,"detached":true});
        if let Handle::Write(mut writer, _paths) = entry {
            let reason = ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::RecoveryRequired,
                "SMB handle detached without saving; retained work requires reconciliation",
            );
            retain_detached(&mut writer, &self.identity.principal, &self.branch, &reason);
            result["operation_id"] = json!(writer.operation_id);
        }
        Ok(result)
    }
}

// Both Flush and terminal Close publish the same borrowed writer. Callers hold
// account admission and the existing path reservation throughout this function.
fn flush_writer(
    shared: &Shared,
    identity: &Identity,
    branch: &str,
    max_file_bytes: u64,
    writer: &mut Writer,
) -> Result<Value, ApiError> {
    if writer.committed {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::WorkspaceRebaseRequired,
            "changed bytes need an authorized workspace before saving",
        ));
    }
    // A lost response must not restage under a new dirty generation and
    // thereby change the original request digest. Reconcile by operation id.
    match shared
        .backend
        .operation_outcome(identity, writer.operation_id)
    {
        Ok(result) if result.commit_state == storage_contract::CommitState::Published => {
            return finish_publication(shared, identity, branch, writer, result);
        }
        Ok(_) => {
            return Err(ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
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
    if writer.file.metadata().map_err(ApiError::io)?.len() > max_file_bytes {
        return Err(ApiError::limit("file exceeds SMB byte limit"));
    }
    shared.backend.upload(
        identity,
        &writer.workspace.workspace_id,
        &writer.item_id,
        &writer.path,
        &crate::UploadSource::from_file(
            writer.file.try_clone().map_err(ApiError::io)?,
            max_file_bytes,
        )?,
    )?;
    let result = shared
        .backend
        .commit_workspace(identity, writer.save_request())?;
    finish_publication(shared, identity, branch, writer, result.into())
}
