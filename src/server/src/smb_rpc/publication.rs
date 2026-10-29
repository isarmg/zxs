//! Dirty generations, recovery records and publication acknowledgement.
use super::*;

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

fn cleanup_writer(writer: &Writer) -> Result<(), ApiError> {
    let result = (|| {
        let name = platform_io::fs_safety::EntryName::new(
            writer
                .source
                .file_name()
                .expect("invariant: writer source has a filename"),
        )
        .map_err(crate::upload_fs_error)?;
        writer
            .directory
            .verify_payload(&name, &writer.file)
            .map_err(crate::upload_fs_error)?;
        writer.reservation.cleanup(budget::Evidence::Writer)
    })();
    if result.is_err() {
        writer.reservation.block_admission();
    }
    result
}

pub(super) fn cleanup_error(mut error: ApiError) -> ApiError {
    error.status = ::http::StatusCode::SERVICE_UNAVAILABLE;
    error.code = storage_contract::ErrorCode::RecoveryRequired;
    error.message = "SMB operation finished but staging cleanup is unconfirmed; query the original operation, do not replay".into();
    error.retryable = false;
    error
}

pub(super) fn finish_publication(
    shared: &Shared,
    identity: &Identity,
    branch: &str,
    writer: &mut Writer,
    result: storage_contract::OperationOutcome,
) -> Result<Value, ApiError> {
    shared.backend.require_native_workspace_commit_completion(
        identity,
        crate::CommitWorkspace {
            workspace_id: writer.workspace.workspace_id.clone(),
            operation_id: writer.operation_id,
            expected_head: writer.workspace.base_head.clone(),
            fencing_token: writer.workspace.fencing_token,
            message: "SMB full-file save".into(),
            request_digest: None,
        },
    )?;
    writer.last_head = result
        .commit_id
        .clone()
        .expect("invariant: published workspace operation identifies its commit");
    // Bytes may have changed after a response was lost. Never acknowledge the
    // earlier publication as if it included those later FD-backed writes.
    // The writer's own path is a point-read hint; a concurrent rename keeps
    // resolving through the descriptor's stable entry identity.
    let entry = shared
        .backend
        .snapshot_entry_by_identity(
            identity,
            branch,
            &writer.last_head,
            writer.path.key(),
            &writer.item_id,
        )?
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
        let Some(Handle::Write(writer)) = self.handles.get_mut(&handle) else {
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
        self.shared
            .backend
            .authorize_current(&self.identity, &self.branch)?;
        // Even an already published handle remains bound to the selection it
        // opened. A same-content default switch must not retarget this FD.
        self.shared.backend.authorize_workspace_selection(
            &self.identity,
            &captured_workspace,
            &path,
        )?;
        let new_workspace = if needs_workspace {
            let workspace = self.workspace()?;
            let validate = (|| {
                let lookup = name_policy::LookupPath::from(&path);
                let previous = self.shared.backend.lookup_at(
                    &self.identity,
                    &self.branch,
                    &last_head,
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
            Some(workspace)
        } else {
            if let Some(Handle::Write(writer)) = self.handles.get(&handle) {
                self.shared.backend.authorize_upload(
                    &self.identity,
                    &writer.workspace.workspace_id,
                    None,
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
            writer.operation_id = OperationId::generate();
            recovery_record(writer, &self.identity.principal, &self.branch)?;
        }
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
                ::http::StatusCode::NOT_FOUND,
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
        match self
            .shared
            .backend
            .operation_outcome(&self.identity, writer.operation_id)
        {
            Ok(Some(result)) if result.commit_state == storage_contract::CommitState::Published => {
                return finish_publication(
                    &self.shared,
                    &self.identity,
                    &self.branch,
                    writer,
                    result,
                );
            }
            Ok(Some(_)) => {
                return Err(ApiError::new(
                    ::http::StatusCode::SERVICE_UNAVAILABLE,
                    storage_contract::ErrorCode::RecoveryRequired,
                    "SMB save outcome must be reconciled before retrying",
                ));
            }
            Ok(None) => {}
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
        let result = self.shared.backend.commit_workspace(
            &self.identity,
            crate::CommitWorkspace {
                workspace_id: writer.workspace.workspace_id.clone(),
                operation_id: writer.operation_id,
                expected_head: writer.workspace.base_head.clone(),
                fencing_token: writer.workspace.fencing_token,
                message: "SMB full-file save".into(),
                request_digest: None,
            },
        )?;
        finish_publication(
            &self.shared,
            &self.identity,
            &self.branch,
            writer,
            result.into(),
        )
    }

    /// Unlike Flush, Close always consumes the handle. A failed publication
    /// becomes retained detached work, not a lock with no VFS owner.
    pub(super) fn close(&mut self, handle: u64, abort: bool) -> Result<Value, ApiError> {
        if !self.handles.contains_key(&handle) {
            return Err(ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "SMB handle missing",
            ));
        }
        let mut saved = (|| {
            if !abort {
                self.flush(handle)?;
            }
            if let Some(Handle::Write(writer)) = self.handles.get(&handle)
                && !writer.committed
            {
                self.shared
                    .backend
                    .abort_workspace(&self.identity, &writer.workspace.workspace_id)?;
            }
            Ok(())
        })();
        if let Some(Handle::Write(mut writer)) = self.handles.remove(&handle) {
            match &saved {
                Ok(()) => {
                    saved = cleanup_writer(&writer).map_err(cleanup_error);
                }
                Err(error) => {
                    retain_detached(&mut writer, &self.identity.principal, &self.branch, error);
                }
            }
            self.release_paths(&[writer.path.key().to_owned()]);
        }
        saved?;
        Ok(json!({"closed":true,"aborted":abort}))
    }

    /// Used when a local worker failed before Close could safely attempt a save.
    /// Detach is not Abort: no backend mutation, source deletion or new operation.
    pub(super) fn detach(&mut self, handle: u64) -> Result<Value, ApiError> {
        let entry = self.handles.remove(&handle).ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "SMB handle missing",
            )
        })?;
        let mut result = json!({"closed":true,"detached":true});
        if let Handle::Write(mut writer) = entry {
            let reason = ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::RecoveryRequired,
                "SMB handle detached without saving; retained work requires reconciliation",
            );
            retain_detached(&mut writer, &self.identity.principal, &self.branch, &reason);
            self.release_paths(&[writer.path.key().to_owned()]);
            result["operation_id"] = json!(writer.operation_id);
        }
        Ok(result)
    }
}
