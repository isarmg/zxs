//! The mutable FD is private staging, not the shared draft. Only an accepted
//! file.draft.write publishes those bytes. Local records retain uncertain work
//! and quota; no constructor reads them to claim a new handle or permission.
use crate::{ApiError, ChangeFileWorking, Identity, SmbDraftAccess, SmbManualFile, UploadSource};
use crate::smb_rpc::{Handle, Session, Shared, budget, path_reservation, recovery_record};
use data_serialization::Serialize;
use json_codec::{Value, json};
use platform_io::fs_safety::{EntryName, PayloadOpenMode, PrivateDirectory};
use smb_protocol::protocol::OpenResult;
use std::{fs::File, io::{Read, Write}, os::fd::OwnedFd, path::Path};
use storage_contract::OperationId;
use storage_contract::file_draft::DraftCommand;

pub(super) mod layout;

#[derive(Serialize)]
#[serialization(tag = "state", rename_all = "snake_case")]
enum Publication {
    Open { operation_id: OperationId },
    Pending { request: ChangeFileWorking },
    Published { request: ChangeFileWorking },
}

impl Publication {
    fn operation(&self) -> OperationId {
        match self {
            Self::Open { operation_id } => *operation_id,
            Self::Pending { request } | Self::Published { request } => request.operation_id,
        }
    }
}

pub(super) struct ManualWriter {
    selection: SmbManualFile,
    revision: storage_contract::file_draft::WorkingRevision,
    publication: Publication,
    recovery: recovery_record::ActiveRecord,
    reservation: budget::Lease,
    file: File,
    directory: PrivateDirectory,
    name: EntryName,
    confirmed_hash: storage_contract::ContentHash,
    confirmed_size: u64,
    dirty: bool,
    write_through: bool,
}

impl ManualWriter {
    pub(super) fn operation(&self) -> OperationId {
        self.publication.operation()
    }

    pub(super) fn selection(&self) -> &SmbManualFile {
        &self.selection
    }

    fn next_revision(&self) -> Result<storage_contract::file_draft::WorkingRevision, ApiError> {
        self.revision.next().map_err(|source| {
            ApiError::limit("the shared draft revision is exhausted")
                .with_source(crate::http_boundary::ApiErrorSource::WorkingRevision(source))
        })
    }

    fn write_revision(&self) -> Result<storage_contract::file_draft::WorkingRevision, ApiError> {
        match self.publication {
            Publication::Published { .. } => self.next_revision(),
            Publication::Open { .. } | Publication::Pending { .. } => Ok(self.revision),
        }
    }

    fn record(&mut self, principal: &str, detached: bool) -> Result<(), ApiError> {
        #[derive(Serialize)]
        struct Record<'a> {
            format: &'static str,
            principal: &'a str,
            target: &'a storage_contract::file_draft::DraftLocation,
            base: &'a storage_contract::file_draft::DraftBase,
            revision: storage_contract::file_draft::WorkingRevision,
            source: &'a Path,
            publication: &'a Publication,
            dirty: bool,
            detached: bool,
            descriptor_changes_unconfirmed: bool,
            confirmed_hash: storage_contract::ContentHash,
            #[serialization(with = "storage_contract::u64_string")]
            confirmed_size: u64,
        }
        let bytes = json_codec::to_vec(&Record {
            format: "smb-manual-draft-work",
            principal,
            target: &self.selection.target,
            base: &self.selection.base,
            revision: self.revision,
            source: self.name.as_path(),
            publication: &self.publication,
            dirty: self.dirty,
            detached,
            descriptor_changes_unconfirmed: true,
            confirmed_hash: self.confirmed_hash,
            confirmed_size: self.confirmed_size,
        }).map_err(ApiError::invalid_source)?;
        self.recovery.persist(&self.directory, &layout::record(&self.name)?, bytes)
    }

    pub(super) fn prepare_write(
        &mut self, shared: &Shared, identity: &Identity, end: u64, bytes: u64,
    ) -> Result<Value, ApiError> {
        if matches!(self.publication, Publication::Pending { .. }) {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "reconcile the original draft save before changing its staged bytes",
            ));
        }
        shared.backend.authorize_smb_draft(
            identity, &self.selection,
            SmbDraftAccess::Write { expected: self.write_revision()? },
        )?;
        let length = self.file.metadata().map_err(ApiError::io)?.len();
        self.reservation.before_write(end, bytes.max(end.saturating_sub(length)))?;
        Ok(json!({"ready":true}))
    }

    pub(super) fn dirty(
        &mut self, shared: &Shared, identity: &Identity, maximum: u64,
    ) -> Result<Value, ApiError> {
        self.dirty = true;
        self.record(&identity.principal, false)?;
        if matches!(self.publication, Publication::Pending { .. }) {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "the original draft save must be reconciled first",
            ));
        }
        let expected = self.write_revision()?;
        shared.backend.authorize_smb_draft(
            identity, &self.selection, SmbDraftAccess::Write { expected },
        )?;
        if matches!(self.publication, Publication::Published { .. }) {
            self.revision = expected;
            self.publication = Publication::Open {
                operation_id: OperationId::generate().map_err(ApiError::contract_input)?,
            };
            self.record(&identity.principal, false)?;
        }
        if self.file.metadata().map_err(ApiError::io)?.len() > maximum {
            return Err(ApiError::limit("file exceeds SMB byte limit"));
        }
        if self.write_through {
            self.flush(shared, identity, maximum)
        } else {
            Ok(json!({"dirty":true}))
        }
    }

    pub(super) fn flush(
        &mut self, shared: &Shared, identity: &Identity, maximum: u64,
    ) -> Result<Value, ApiError> {
        if matches!(self.publication, Publication::Pending { .. }) {
            // Reconcile the original receipt before today's selection/revision.
            // The durable writer still admits this exact share and principal.
            shared.backend.authorize_repository(
                identity, &self.selection.target.repository.to_string(),
            )?;
        } else {
            shared.backend.authorize_smb_draft(identity, &self.selection, SmbDraftAccess::Read)?;
        }
        self.directory.verify_payload(&self.name, &self.file).map_err(crate::upload_fs_error)?;
        self.file.sync_all().map_err(ApiError::io)?;
        let source = UploadSource::from_file(
            self.file.try_clone().map_err(ApiError::io)?, maximum,
        )?;
        if matches!(self.publication, Publication::Published { .. }) {
            // This is custody of an already-published operation, not comparison
            // against a historical version. A lost Dirty RPC must not discard
            // later descriptor bytes on a repeat Flush or terminal Close.
            if !self.dirty && source.hash == self.confirmed_hash && source.size == self.confirmed_size {
                return Ok(json!({"saved":true,"changed":false}));
            }
            self.dirty = true;
            self.record(&identity.principal, false)?;
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::FileBranchConflict,
                "saved draft differs from later FD bytes; authorize a new dirty generation",
            ));
        }
        // The writable descriptor may have changed without a Dirty notice.
        // Persist its first save as a draft without looking at the initial bytes.
        if let Publication::Open { operation_id } = self.publication {
            // Retain the original revision, selection and full byte commitment
            // before invoking any durable writer. No later retry re-hashes it
            // into a different request under this operation id.
            self.next_revision()?;
            self.dirty = true;
            self.publication = Publication::Pending {
                request: ChangeFileWorking {
                    operation_id,
                    command: DraftCommand::Write {
                        controller: self.selection.working.save_controller(),
                        target: self.selection.target.clone(),
                        expected: self.revision,
                        base: self.selection.base.clone(),
                        content_hash: source.hash,
                        logical_size: source.size,
                    },
                },
            };
        }
        self.record(&identity.principal, false)?;
        let request = match &self.publication {
            Publication::Pending { request } => request.clone(),
            Publication::Open { .. } | Publication::Published { .. } => {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::RecoveryRequired,
                    "manual draft save lacks its frozen request",
                ));
            }
        };
        let result = shared.backend.write_smb_draft(identity, request.clone(), &source)?;
        if result.commit_state != storage_contract::CommitState::Published {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "manual draft publication is not complete",
            ));
        }
        let latest = UploadSource::from_file(
            self.file.try_clone().map_err(ApiError::io)?, maximum,
        )?;
        let DraftCommand::Write { content_hash, logical_size, .. } = &request.command else {
            return Err(ApiError::forbidden());
        };
        self.dirty = latest.hash != *content_hash || latest.size != *logical_size;
        self.confirmed_hash = *content_hash;
        self.confirmed_size = *logical_size;
        self.publication = Publication::Published { request };
        self.record(&identity.principal, false)?;
        if self.dirty {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::FileBranchConflict,
                "original draft saved; later FD bytes remain unsaved",
            ));
        }
        crate::operation_wire::operation_wire(result.into())
    }

    pub(super) fn retain(&mut self, principal: &str) {
        if let Err(error) = self.file.sync_all().map_err(ApiError::io)
            .and_then(|()| self.record(principal, true))
        {
            tracing::warn!(%error, operation_id = %self.operation(),
                "manual SMB work remains retained after failed detachment sync");
        }
    }

    pub(super) fn close(
        &mut self, shared: &Shared, identity: &Identity, maximum: u64, abort: bool,
    ) -> Result<(), ApiError> {
        let result = (|| {
            if !abort {
                self.flush(shared, identity, maximum)?;
            } else {
                shared.backend.authorize_smb_draft(
                    identity, &self.selection, SmbDraftAccess::Read,
                )?;
                if matches!(self.publication, Publication::Pending { .. }) {
                    return Err(ApiError::conflict(
                        storage_contract::ErrorCode::RecoveryRequired,
                        "an uncertain draft save cannot be aborted by deleting its source",
                    ));
                }
            }
            self.directory.verify_payload(&self.name, &self.file)
                .map_err(crate::upload_fs_error)?;
            self.recovery.remove(&self.directory, &layout::record(&self.name)?)?;
            self.directory.remove_file(&self.name).map_err(crate::upload_fs_error)
        })();
        if result.is_err() {
            self.retain(&identity.principal);
        }
        result
    }
}

impl Session {
    pub(super) fn open_manual(
        &mut self, handle: u64, file: SmbManualFile, writable: bool,
        truncate: bool, write_through: bool,
    ) -> Result<(OpenResult, OwnedFd), ApiError> {
        let mtime = self.version_mtime(&file.base.version_id().to_string())?;
        let version = if file.working.draft().is_some() {
            None
        } else {
            Some(file.base.version_id().to_string())
        };
        let path = file.target.path.clone();
        let entry_id = file.target.entry.to_string();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        let (opened, descriptor, size) = if writable {
            let paths = path_reservation::PathReservation::acquire(
                &self.shared, [(self.branch.as_str(), path.key())],
            )?;
            self.shared.backend.authorize_smb_draft(
                &self.identity, &file, SmbDraftAccess::Write { expected: file.revision() },
            )?;
            let operation_id = OperationId::generate().map_err(ApiError::contract_input)?;
            let directory = PrivateDirectory::open_trusted(&self.shared.spool)
                .map_err(crate::upload_fs_error)?;
            let name = layout::source(operation_id)?;
            let content = if truncate {
                None
            } else {
                Some(self.shared.backend.smb_draft_content(&self.identity, &file, deadline)?
                    .fully_verified()?)
            };
            let (confirmed_hash, confirmed_size) = match content.as_ref() {
                Some(content) => (
                    content.hash.parse().map_err(ApiError::invalid_source)?, content.size,
                ),
                None => (storage_contract::ContentHash::from_digest(blake3::hash(&[])), 0),
            };
            let mut reservation = self.shared.budget.reserve(
                &self.identity.principal, &directory.path().join(name.as_path()),
                content.as_ref().map_or(0, |content| content.size), self.max_file_bytes,
            )?;
            let mut source = directory.open_payload(&name, PayloadOpenMode::CreateNew)
                .map_err(crate::upload_fs_error)?;
            if let Some(mut content) = content {
                let mut copied = 0u64;
                let mut digest = blake3::Hasher::new();
                let mut buffer = [0u8; 65_536];
                loop {
                    content.check_running()?;
                    let length = content.file.read(&mut buffer).map_err(ApiError::io)?;
                    if length == 0 {
                        break;
                    }
                    copied = copied.checked_add(length as u64)
                        .ok_or_else(|| ApiError::limit("SMB byte count overflow"))?;
                    reservation.before_write(copied, length as u64)?;
                    digest.update(&buffer[..length]);
                    source.write_all(&buffer[..length]).map_err(ApiError::io)?;
                }
                content.check_running()?;
                if copied != content.size || digest.finalize().to_hex().as_str() != content.hash {
                    return Err(ApiError::conflict(
                        storage_contract::ErrorCode::ChecksumMismatch,
                        "manual draft changed while its private write copy was opening",
                    ));
                }
            }
            source.sync_all().map_err(ApiError::io)?;
            let size = source.metadata().map_err(ApiError::io)?.len();
            let descriptor = OwnedFd::from(source.try_clone().map_err(ApiError::io)?);
            let mut writer = ManualWriter {
                revision: file.revision(),
                selection: file,
                publication: Publication::Open { operation_id },
                recovery: recovery_record::ActiveRecord::default(),
                reservation, file: source, directory, name,
                confirmed_hash, confirmed_size,
                dirty: truncate, write_through,
            };
            writer.record(&self.identity.principal, false)?;
            (Handle::ManualWrite(Box::new(writer), paths), descriptor, size)
        } else {
            let content = self.shared.backend.smb_draft_content(&self.identity, &file, deadline)?;
            let manifest = content.checksum_manifest.as_ref()
                .ok_or_else(|| ApiError::invalid("manual draft content has no range proof"))?;
            crate::verified_chunk::validate_manifest(manifest, content.size)?;
            let size = content.size;
            // Same non-readable placeholder as ordinary verified range reads.
            let descriptor = OwnedFd::from(
                File::open(layout::read_placeholder(&self.shared.spool)).map_err(ApiError::io)?,
            );
            (Handle::ReadDraft(content, file), descriptor, size)
        };
        self.handles.insert(handle, opened);
        Ok((OpenResult {
            data_io: if writable { smb_protocol::protocol::OpenIo::Descriptor }
                else { smb_protocol::protocol::OpenIo::VerifiedRanges },
            entry_id, kind: storage_contract::NamespaceEntryKind::File,
            path: path.into_string(), handle, size, max_file_bytes: self.max_file_bytes,
            version_id: version, workspace_id: None, mtime,
        }, descriptor))
    }
}
