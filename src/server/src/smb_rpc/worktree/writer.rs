//! Existing-file replacement saves share the sealed core edit publisher. The
//! private mutable descriptor is never a worktree leaf or a history version.
use crate::smb_rpc::{Shared, budget, recovery_record};
use crate::smb_rpc::worktree::layout;
use crate::{ApiError, Identity, SmbWorktreeAccess, SmbWorktreeFile, UploadSource};
use data_serialization::Serialize;
use json_codec::{Value, json};
use platform_io::fs_safety::{EntryName, PrivateDirectory};
use std::{fs::File, path::Path};
use storage_contract::snapshot_worktree::{EditWorktree, WorktreeRevision};
use storage_contract::{ContentHash, ErrorCode, OperationId, Principal};

#[derive(Serialize)]
#[serialization(tag = "state", rename_all = "snake_case")]
enum Publication {
    Open { operation: OperationId },
    Pending { request: EditWorktree },
    Published { request: EditWorktree },
}

pub(in crate::smb_rpc) struct WorktreeWriter {
    pub(super) selection: SmbWorktreeFile,
    principal: Principal,
    publication: Publication,
    recovery: recovery_record::ActiveRecord,
    reservation: budget::Lease,
    file: File,
    directory: PrivateDirectory,
    name: EntryName,
    confirmed_hash: ContentHash,
    confirmed_size: u64,
    dirty: bool,
    write_through: bool,
}

pub(super) struct Preparation {
    pub selection: SmbWorktreeFile,
    pub operation: OperationId,
    pub reservation: budget::Lease,
    pub file: File,
    pub directory: PrivateDirectory,
    pub name: EntryName,
    pub hash: ContentHash,
    pub size: u64,
    pub truncated: bool,
    pub write_through: bool,
}

impl WorktreeWriter {
    pub(super) fn new(input: Preparation, identity: &Identity) -> Result<Self, ApiError> {
        let mut writer = Self {
            selection: input.selection,
            principal: identity.principal.parse()
                .map_err(ApiError::invalid_authenticated_principal)?,
            publication: Publication::Open { operation: input.operation },
            recovery: recovery_record::ActiveRecord::default(), reservation: input.reservation,
            file: input.file, directory: input.directory, name: input.name,
            confirmed_hash: input.hash, confirmed_size: input.size,
            dirty: input.truncated, write_through: input.write_through,
        };
        writer.record(false)?;
        Ok(writer)
    }

    pub(in crate::smb_rpc) fn operation(&self) -> OperationId {
        match &self.publication {
            Publication::Open { operation } => *operation,
            Publication::Pending { request } | Publication::Published { request } => {
                request.operation_id
            }
        }
    }

    pub(in crate::smb_rpc) fn target(&self) -> storage_contract::snapshot_worktree::WorktreeHandle {
        self.selection.target
    }

    pub(in crate::smb_rpc) fn visible(&self) -> OperationId { self.selection.visible }

    fn authorize_write(&self, shared: &Shared, identity: &Identity) -> Result<(), ApiError> {
        shared.backend.authorize_smb_worktree(identity, self.selection.target,
            SmbWorktreeAccess::Write {
                visible: self.selection.visible,
                path: &self.selection.path, expected: self.selection.revision,
            })
    }

    fn record(&mut self, detached: bool) -> Result<(), ApiError> {
        #[derive(Serialize)]
        struct Record<'a> {
            format: &'static str,
            principal: &'a Principal,
            target: storage_contract::snapshot_worktree::WorktreeHandle,
            path: &'a name_policy::PortablePath,
            expected: WorktreeRevision,
            source: &'a Path,
            publication: &'a Publication,
            dirty: bool,
            detached: bool,
            descriptor_changes_unconfirmed: bool,
        }
        let bytes = json_codec::to_vec(&Record {
            format: "smb-worktree-replacement", principal: &self.principal,
            target: self.selection.target,
            path: &self.selection.path, expected: self.selection.revision,
            source: self.name.as_path(), publication: &self.publication,
            dirty: self.dirty, detached, descriptor_changes_unconfirmed: true,
        }).map_err(ApiError::invalid_source)?;
        self.recovery.persist(&self.directory, &layout::record(&self.name)?, bytes)
    }

    pub(in crate::smb_rpc) fn prepare_write(
        &mut self, shared: &Shared, identity: &Identity, end: u64, bytes: u64,
    ) -> Result<Value, ApiError> {
        if matches!(self.publication, Publication::Pending { .. }) {
            return Err(pending());
        }
        self.authorize_write(shared, identity)?;
        let length = self.file.metadata().map_err(ApiError::io)?.len();
        self.reservation.before_write(end, bytes.max(end.saturating_sub(length)))?;
        Ok(json!({"ready": true}))
    }

    pub(in crate::smb_rpc) fn dirty(
        &mut self, shared: &Shared, identity: &Identity, maximum: u64,
    ) -> Result<Value, ApiError> {
        self.dirty = true;
        self.record(false)?;
        if matches!(self.publication, Publication::Pending { .. }) {
            return Err(pending());
        }
        self.authorize_write(shared, identity)?;
        if matches!(self.publication, Publication::Published { .. }) {
            self.publication = Publication::Open {
                operation: OperationId::generate().map_err(ApiError::contract_input)?,
            };
            self.record(false)?;
        }
        if self.file.metadata().map_err(ApiError::io)?.len() > maximum {
            return Err(ApiError::limit("worktree file exceeds SMB byte budget"));
        }
        if self.write_through {
            self.flush(shared, identity, maximum)
        } else {
            Ok(json!({"dirty": true}))
        }
    }

    pub(in crate::smb_rpc) fn flush(
        &mut self, shared: &Shared, identity: &Identity, maximum: u64,
    ) -> Result<Value, ApiError> {
        shared.backend.authorize_smb_worktree(
            identity, self.selection.target, SmbWorktreeAccess::Read { visible: self.selection.visible },
        )?;
        self.directory.verify_payload(&self.name, &self.file).map_err(crate::upload_fs_error)?;
        self.file.sync_all().map_err(ApiError::io)?;
        let source = UploadSource::from_file(
            self.file.try_clone().map_err(ApiError::io)?, maximum,
        )?;
        if !matches!(self.publication, Publication::Pending { .. })
            && !self.dirty && source.hash == self.confirmed_hash
            && source.size == self.confirmed_size
        {
            return Ok(json!({"saved": true, "changed": false}));
        }
        if matches!(self.publication, Publication::Published { .. }) {
            self.dirty = true;
            self.record(false)?;
            return Err(ApiError::conflict(ErrorCode::PreparationStale,
                "later descriptor bytes need an authorized Dirty generation"));
        }
        if let Publication::Open { operation } = self.publication {
            self.authorize_write(shared, identity)?;
            self.publication = Publication::Pending { request: EditWorktree {
                target: self.selection.target, path: self.selection.path.clone(),
                expected: self.selection.revision, size: source.size, content_hash: source.hash,
                operation_id: operation,
            }};
            self.dirty = true;
            self.record(false)?;
        }
        let Publication::Pending { request } = &self.publication else {
            return Err(pending());
        };
        let request = request.clone();
        let revision = shared.backend.edit_smb_worktree(identity, request.clone(), self.selection.visible, &source)?;
        // Descriptor mutation is outside the engine lock. Never acknowledge
        // later bytes as part of an older completed operation.
        let latest = UploadSource::from_file(
            self.file.try_clone().map_err(ApiError::io)?, maximum,
        )?;
        self.dirty = latest.hash != request.content_hash || latest.size != request.size;
        self.confirmed_hash = request.content_hash;
        self.confirmed_size = request.size;
        self.selection.revision = revision;
        self.publication = Publication::Published { request };
        self.record(false)?;
        if self.dirty {
            return Err(ApiError::conflict(ErrorCode::PreparationStale,
                "original worktree save completed; later descriptor bytes remain unsaved"));
        }
        Ok(json!({"saved": true, "revision": revision, "operation_id": self.operation()}))
    }

    pub(in crate::smb_rpc) fn retain(&mut self) {
        if let Err(error) = self.file.sync_all().map_err(ApiError::io)
            .and_then(|()| self.record(true))
        {
            tracing::warn!(%error, operation_id = %self.operation(),
                "detached SMB worktree source remains retained");
        }
    }

    pub(in crate::smb_rpc) fn close(
        &mut self, shared: &Shared, identity: &Identity, maximum: u64, abort: bool,
    ) -> Result<(), ApiError> {
        let result = (|| {
            if abort {
                shared.backend.authorize_smb_worktree(
                    identity, self.selection.target, SmbWorktreeAccess::Read { visible: self.selection.visible },
                )?;
                if matches!(self.publication, Publication::Pending { .. }) {
                    return Err(pending());
                }
            } else {
                self.flush(shared, identity, maximum)?;
            }
            self.directory.verify_payload(&self.name, &self.file)
                .map_err(crate::upload_fs_error)?;
            self.recovery.remove(&self.directory, &layout::record(&self.name)?)?;
            self.directory.remove_file(&self.name).map_err(crate::upload_fs_error)
        })();
        if result.is_err() { self.retain(); }
        result
    }
}

fn pending() -> ApiError {
    ApiError::conflict(ErrorCode::RecoveryRequired,
        "retain the original worktree save and reconcile it before changing staged bytes")
}
