//! Added files retain the native draft owner and reuse its ordered I/O handle.
//! A pre-publication intent fences another birth; it is never a success receipt.
use crate::smb_rpc::namespace::NamespaceAdmission;
use crate::smb_rpc::worktree::{display_path, layout};
use crate::smb_rpc::{Session, path_reservation};
use crate::{ApiError, SmbWorktreeWorkingOpen, SmbWorktreeWorkingSource};
use data_serialization::{Deserialize, Serialize};
use name_policy::{LookupPath, Name, PortablePath};
use platform_io::fs_safety::PrivateDirectory;
use platform_io::fs_safety::linux::{MountPolicy, OpenAt2Root};
use smb_protocol::protocol::OpenResult;
use std::os::fd::OwnedFd;
use storage_contract::snapshot_worktree::additions::{CreateWorktreeFile, WorktreeFileTarget};
use storage_contract::snapshot_worktree::{WorktreeHandle, WorktreeStatus};
use storage_contract::{ErrorCode, NamespaceEntryKind, OperationId, Principal};

const MAX_JOURNAL_ENTRIES: usize = 4096;
const MAX_JOURNAL_BYTES: usize = 4 * 1024 * 1024;
const MAX_INTENT_BYTES: usize = 64 * 1024;

#[derive(Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
enum IntentFormat {
    SmbWorktreeFileCreate,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Intent {
    format: IntentFormat,
    principal: Principal,
    request: CreateWorktreeFile,
}

impl Session {
    pub(super) fn create_worktree_working(
        &mut self,
        status: &WorktreeStatus,
        path: &LookupPath,
        writable: bool,
        truncate: bool,
        write_through: bool,
    ) -> Result<(OpenResult, OwnedFd), ApiError> {
        let path = if let Some((parent, leaf)) = path.as_str().rsplit_once('/') {
            let parent = LookupPath::parse(parent).map_err(ApiError::invalid_source)?;
            let parent = self.shared.backend.smb_worktree_entry(
                &self.identity, status.target, &parent,
            )?;
            if parent.kind() != NamespaceEntryKind::Directory {
                return Err(ApiError::conflict(
                    ErrorCode::NotDirectory,
                    "worktree file parent is not a directory",
                ));
            }
            let name = Name::parse(leaf).map_err(ApiError::invalid_source)?;
            parent.path.join_name(&name).map_err(ApiError::invalid_source)?
        } else {
            PortablePath::try_from(path.clone()).map_err(ApiError::invalid_source)?
        };
        let display = display_path(&status.name, Some(&path))?;
        let mut birth_path = Some(path_reservation::PathReservation::acquire(
            &self.shared,
            [(self.branch.as_str(), display.key())],
        )?);
        let principal = self
            .identity
            .principal
            .parse()
            .map_err(ApiError::invalid_authenticated_principal)?;
        require_no_retained_birth(&self.shared.spool, &principal, status.target, &path)?;
        let operation_id = OperationId::generate().map_err(ApiError::contract_input)?;
        let intent = Intent {
            format: IntentFormat::SmbWorktreeFileCreate,
            principal,
            request: CreateWorktreeFile {
                target: status.target,
                path,
                expected: status.revision,
                operation_id,
            },
        };
        let admission = NamespaceAdmission::acquire(&self.shared, &self.identity, operation_id)?;
        admission.record_intent(&intent).map_err(|mut error| {
            error.retryable = false;
            error.message = format!(
                "{}; worktree birth {} was not submitted; retain any original recovery intent",
                error.message, operation_id,
            );
            error
        })?;
        let result = (|| {
            let file = self.shared.backend.smb_worktree_working_open(
                &self.identity,
                SmbWorktreeWorkingOpen {
                    source: SmbWorktreeWorkingSource::Create(intent.request),
                    writable,
                    truncate,
                },
            )?;
            // Every birth owns its path until admission is finished. Only a
            // writable open continues to own that writer slot until close.
            let writer_path = if writable { birth_path.take() } else { None };
            self.install_native(file, &display, writable, write_through, writer_path)
        })();
        match result {
            Ok(opened) => {
                // Both birth and the exact native open completed. A failed open
                // retains this reference even if birth already became durable.
                admission.finish_published();
                Ok(opened)
            }
            Err(mut error) => {
                error.retryable = false;
                error.message = format!(
                    "{}; retain original worktree birth {}",
                    error.message, operation_id,
                );
                Err(error)
            }
        }
    }

    pub(super) fn open_worktree_working(
        &mut self,
        target: WorktreeFileTarget,
        display: &PortablePath,
        writable: bool,
        truncate: bool,
        write_through: bool,
    ) -> Result<(OpenResult, OwnedFd), ApiError> {
        let paths = if writable {
            Some(path_reservation::PathReservation::acquire(
                &self.shared,
                [(self.branch.as_str(), display.key())],
            )?)
        } else {
            None
        };
        let file = self.shared.backend.smb_worktree_working_open(
            &self.identity,
            SmbWorktreeWorkingOpen {
                source: SmbWorktreeWorkingSource::Existing(target),
                writable,
                truncate,
            },
        )?;
        self.install_native(file, display, writable, write_through, paths)
    }
}

fn require_no_retained_birth(
    spool: &std::path::Path,
    principal: &Principal,
    target: WorktreeHandle,
    path: &PortablePath,
) -> Result<(), ApiError> {
    #[derive(Deserialize)]
    struct Header {
        format: Option<String>,
    }

    let directory = PrivateDirectory::open_trusted(spool).map_err(crate::upload_fs_error)?;
    let root = OpenAt2Root::from_private_directory(
        directory.try_clone().map_err(crate::upload_fs_error)?,
        MountPolicy::SameMount,
    )
    .map_err(crate::upload_fs_error)?;
    let mut remaining = MAX_JOURNAL_BYTES;
    for (index, name) in root.entries().map_err(crate::upload_fs_error)?.enumerate() {
        if index >= MAX_JOURNAL_ENTRIES {
            return Err(ApiError::limit(
                "SMB recovery intent inventory exceeds its entry budget",
            ));
        }
        let name = name.map_err(crate::upload_fs_error)?;
        if !layout::is_namespace_record(&name) {
            continue;
        }
        let bytes = directory
            .read_bounded(&name, MAX_INTENT_BYTES.min(remaining))
            .map_err(crate::upload_fs_error)?;
        remaining -= bytes.len();
        let header: Header = json_codec::from_slice(&bytes).map_err(ApiError::invalid_source)?;
        if header.format.as_deref() != Some("smb_worktree_file_create") {
            continue;
        }
        let intent: Intent =
            storage_contract::decode_strict(&bytes).map_err(ApiError::contract_input)?;
        if name != layout::namespace_record(intent.request.operation_id)? {
            return Err(ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "retained worktree birth filename does not match its original operation",
            ));
        }
        if intent.request.target == target && intent.request.path.key() == path.key() {
            let message = if &intent.principal == principal {
                format!(
                    "retain and reconcile original worktree birth {}",
                    intent.request.operation_id
                )
            } else {
                "this worktree path has a retained birth owned by another principal".to_owned()
            };
            let mut error = ApiError::conflict(ErrorCode::RecoveryRequired, &message);
            error.retryable = false;
            return Err(error);
        }
    }
    directory.verify_identity().map_err(crate::upload_fs_error)
}
