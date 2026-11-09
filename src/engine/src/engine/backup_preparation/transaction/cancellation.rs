//! Cancel only unselected preparation. Preserve the complete SYSTEM task and
//! every target byte; neither an archive nor a tombstone grants disk authority.
use super::*;
use crate::history_package::{install_preparation_bytes, preparation_path_exists};
use std::path::Path;

const ACTIVE: &str = "backup-preparation";
const ARCHIVES: &str = "backup-preparation-archive";
const INTENT: &str = "cancellation.json";
const COMPLETE: &str = "cancelled.json";
const FORMAT: &str = "zxs-backup-preparation-cancellation";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupTransactionCancellation {
    Pending,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupTransactionCancelOutcome {
    Cancelled,
    /// No cancellation intent was installed. Preserve this original operation
    /// and use its native continuation; never start a replacement transaction.
    ContinuationRequired,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Cancellation {
    format: String,
    request: BackupTransactionRequest,
}
impl Cancellation {
    fn validate(&self) -> Result<()> {
        Engine::require_actor(&self.request.preparation.principal)?;
        let count = self.request.preparation.target_paths.len();
        // A rejected Start may contain a stale sequence or invalid topology.
        // This bounded input only forbids its op ID from starting later. None
        // of its paths is opened or used as an authority by cancellation.
        if self.format != FORMAT || !(1..=8).contains(&count) || count != self.request.targets.len()
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "invalid original Backup cancellation record",
            ));
        }
        Ok(())
    }
    fn require_actor(&self, operation: OperationId, principal: &str) -> Result<()> {
        Engine::require_actor(principal)?;
        if self.request.preparation.operation_id != operation
            || self.request.preparation.principal != principal
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "Backup cancellation belongs to another original operation or principal",
            ));
        }
        Ok(())
    }
    fn bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = canonical_json(self)?;
        if bytes.len() > MAX_REQUEST {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "Backup cancellation exceeds 64 KiB",
            ));
        }
        Ok(bytes)
    }
    fn read(system: &DiskRoot, directory: &Path) -> Result<Option<Self>> {
        let path = directory.join(INTENT);
        if !preparation_path_exists(system, &path)? {
            return Ok(None);
        }
        let saved: Self = decode_strict(&system.read_bytes(&path, MAX_REQUEST)?)?;
        saved.validate()?;
        Ok(Some(saved))
    }
}

fn archive(system: &DiskRoot, operation: OperationId) -> PathBuf {
    system.path().join(ARCHIVES).join(operation.to_string())
}
fn active_cancellation(system: &DiskRoot) -> Result<Option<Cancellation>> {
    let Some(cancel) = Cancellation::read(system, &system.path().join(ACTIVE))? else {
        return Ok(None);
    };
    let saved = Retained::read(system)?.ok_or_else(|| {
        Error::new(
            ErrorCode::ConfigConflict,
            "cancellation has no exact retained Backup request",
        )
    })?;
    if saved.request != cancel.request {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "cancellation differs from the retained Backup request",
        ));
    }
    Ok(Some(cancel))
}
fn archived(system: &DiskRoot, operation: OperationId) -> Result<Option<Cancellation>> {
    let directory = archive(system, operation);
    let Some(cancel) = Cancellation::read(system, &directory)? else {
        return Ok(None);
    };
    if cancel.request.preparation.operation_id != operation {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "Backup archive does not bind its original operation",
        ));
    }
    if Retained::read(system)?
        .is_some_and(|saved| saved.request.preparation.operation_id == operation)
    {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "original Backup task is present in both active and archived namespaces",
        ));
    }
    Ok(Some(cancel))
}
pub(super) fn archived_request(
    system: &DiskRoot,
    operation: OperationId,
    principal: &str,
) -> Result<Option<BackupTransactionRequest>> {
    let Some(cancel) = archived(system, operation)? else {
        return Ok(None);
    };
    cancel.require_actor(operation, principal)?;
    Ok(Some(cancel.request))
}
pub(in crate::engine::backup_preparation) fn cancellation_pending(
    system: &DiskRoot,
) -> Result<bool> {
    Ok(active_cancellation(system)?.is_some())
}
pub(in crate::engine::backup_preparation) fn require_not_cancelling(
    system: &DiskRoot,
) -> Result<()> {
    if cancellation_pending(system)? {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "original Backup cancellation is pending; retry cancellation, not preparation or fencing",
        ));
    }
    Ok(())
}
pub(in crate::engine::backup_preparation) fn require_operation_open(
    system: &DiskRoot,
    operation: OperationId,
) -> Result<()> {
    require_not_cancelling(system)?;
    if preparation_path_exists(system, &archive(system, operation))? {
        return Err(Error::new(
            ErrorCode::IdempotencyKeyReused,
            "cancelled Backup operation cannot be restarted; retain its bytes and choose a new operation",
        ));
    }
    Ok(())
}

impl Engine {
    pub fn backup_transaction_cancellation(
        &self,
        operation: OperationId,
        principal: &str,
    ) -> Result<Option<BackupTransactionCancellation>> {
        Self::require_actor(principal)?;
        let system = DiskRoot::open(&self.root)?;
        if let Some(cancel) = archived(&system, operation)? {
            cancel.require_actor(operation, principal)?;
            let path = archive(&system, operation).join(COMPLETE);
            if !preparation_path_exists(&system, &path)? {
                return Ok(Some(BackupTransactionCancellation::Pending));
            }
            if system.read_bytes(&path, MAX_REQUEST)? != cancel.bytes()? {
                return Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "Backup cancellation completion differs from its original intent",
                ));
            }
            return Ok(Some(BackupTransactionCancellation::Cancelled));
        }
        if let Some(cancel) = active_cancellation(&system)? {
            cancel.require_actor(operation, principal)?;
            return Ok(Some(BackupTransactionCancellation::Pending));
        }
        Ok(None)
    }

    /// Stop and join only this unselected preparation, then preserve its entire
    /// SYSTEM directory by a no-clobber rename. No target is opened or deleted.
    /// `original` also cancels a Start which never reached SYSTEM, preventing a
    /// delayed request from activating that same ID after the user starts anew.
    pub fn cancel_backup_transaction(
        &mut self,
        operation: OperationId,
        principal: &str,
        original: Option<BackupTransactionRequest>,
    ) -> Result<BackupTransactionCancelOutcome> {
        Self::require_actor(principal)?;
        self.require_no_commit_preparation()?;
        let system = DiskRoot::open(&self.root)?;
        if let Some(cancel) = archived(&system, operation)? {
            cancel.require_actor(operation, principal)?;
            require_original(&cancel.request, original.as_ref())?;
            self.finish_backup_cancellation(&system, &cancel)?;
            return Ok(BackupTransactionCancelOutcome::Cancelled);
        }
        let saved = Retained::read(&system)?;
        let owns_active = saved
            .as_ref()
            .is_some_and(|saved| saved.request.preparation.operation_id == operation);
        let request = if owns_active {
            let saved = saved.expect("invariant: matched retained operation");
            saved.require_actor(operation, principal)?;
            require_original(&saved.request, original.as_ref())?;
            saved.request
        } else {
            let original = original.ok_or_else(|| {
                Error::new(
                    ErrorCode::NotFound,
                    "cancelling an unobserved Start requires that exact original input",
                )
            })?;
            if original.preparation.operation_id != operation
                || original.preparation.principal != principal
            {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "cancellation cannot substitute an operation or principal",
                ));
            }
            if saved.is_none()
                && (self.backup_preparation.is_some()
                    || preparation_path_exists(&system, &system.path().join(JOB))?
                    || preparation_path_exists(&system, &system.path().join(batch::BATCH_JOB))?)
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "a legacy preparation without its user request cannot be cancelled by replacement input",
                ));
            }
            original
        };
        let cancel = Cancellation {
            format: FORMAT.into(),
            request,
        };
        let bytes = cancel.bytes()?;
        let pending = owns_active && active_cancellation(&system)?.is_some();
        if self.state.operations.contains_key(&operation) {
            if pending {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "a pending Backup cancellation cannot retire a published operation",
                ));
            }
            return Ok(BackupTransactionCancelOutcome::ContinuationRequired);
        }
        if owns_active {
            let cannot_cancel = self.blocked
                || self.membership_fence.is_some()
                || self.pending_membership.is_some()
                || self.pending_disk_exit.is_some()
                || self.history_move_choice.is_some()
                || self.history_move_prepare.is_some()
                || self.backup_serving.is_some()
                || preparation_path_exists(
                    &system,
                    &system
                        .path()
                        .join("backup-preparation/target-entities.json"),
                )?
                || preparation_path_exists(
                    &system,
                    &system
                        .path()
                        .join("backup-preparation/registry-placement.json"),
                )?;
            if cannot_cancel {
                if pending {
                    return Err(Error::new(
                        ErrorCode::RecoveryRequired,
                        "pending Backup cancellation conflicts with a native stop; preserve the original task",
                    ));
                }
                return Ok(BackupTransactionCancelOutcome::ContinuationRequired);
            }
            if let Some(worker) = &self.backup_preparation {
                let request = worker
                    .batch
                    .as_ref()
                    .map_or(&worker.job.request, |batch| batch.request());
                if request != &cancel.request.preparation {
                    return Err(Error::new(
                        ErrorCode::IdempotencyKeyReused,
                        "running Backup worker belongs to another original request",
                    ));
                }
            }
            install_preparation_bytes(&system, &system.path().join(ACTIVE).join(INTENT), &bytes)?;
            if let Some(worker) = self.backup_preparation.as_mut() {
                worker.stop.store(true, Ordering::Release);
                worker.join()?;
            }
            system.private_dir(&system.path().join(ARCHIVES))?;
            system.rename_noreplace_destination_first_synced(
                &system.path().join(ACTIVE),
                &archive(&system, operation),
            )?;
        } else {
            // This does not move, release, or change another active task. Only
            // the exact rejected/undelivered operation gets an immutable tombstone.
            install_preparation_bytes(&system, &archive(&system, operation).join(INTENT), &bytes)?;
        }
        self.finish_backup_cancellation(&system, &cancel)?;
        Ok(BackupTransactionCancelOutcome::Cancelled)
    }

    fn finish_backup_cancellation(
        &mut self,
        system: &DiskRoot,
        cancel: &Cancellation,
    ) -> Result<()> {
        let operation = cancel.request.preparation.operation_id;
        let actual = archived(system, operation)?.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "the exact Backup cancellation archive is not present",
            )
        })?;
        if actual.bytes()? != cancel.bytes()? {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "Backup cancellation archive changed",
            ));
        }
        if let Some(worker) = self.backup_preparation.as_mut()
            && worker.job.request.operation_id == operation
        {
            let request = worker
                .batch
                .as_ref()
                .map_or(&worker.job.request, |batch| batch.request());
            if request != &cancel.request.preparation {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "archived Backup worker differs from its cancelled input",
                ));
            }
            worker.stop.store(true, Ordering::Release);
            worker.join()?;
        }
        if self
            .backup_preparation
            .as_ref()
            .is_some_and(|worker| worker.job.request.operation_id == operation)
        {
            self.backup_preparation = None;
        }
        // Complete every durability tail even when a previous rename succeeded
        // but returned an I/O error. GET remains pending until this file exists.
        system.sync_dir(&archive(system, operation))?;
        system.sync_dir(&system.path().join(ARCHIVES))?;
        system.sync_dir(system.path())?;
        install_preparation_bytes(
            system,
            &archive(system, operation).join(COMPLETE),
            &cancel.bytes()?,
        )?;
        Ok(())
    }
}

fn require_original(
    saved: &BackupTransactionRequest,
    supplied: Option<&BackupTransactionRequest>,
) -> Result<()> {
    if supplied.is_some_and(|original| original != saved) {
        return Err(Error::new(
            ErrorCode::IdempotencyKeyReused,
            "cancellation must preserve the original complete Backup input",
        ));
    }
    Ok(())
}
