//! Joining a copier is not a test for whether a user-owned Backup task exists.
use super::{BackupPreparationStatus, transaction};
use crate::{
    Error, ErrorCode, OperationId, Result, disk::DiskRoot, engine::Engine, persist::layout,
};
use platform_io::fs_safety::linux::FileIdentity;

#[derive(Debug, error_definition::Error)]
pub(super) enum Failure {
    #[error("no backup preparation")]
    Absent,
    #[error("backup preparation {0} is retained without a worker; inspect the original task")]
    Retained(OperationId),
    #[error("backup preparation worker panicked for {0}; durable task and original data retained")]
    WorkerPanicked(OperationId),
    #[error("original SYSTEM directory or writer changed while checking Backup shutdown")]
    SystemChanged,
}

impl From<Failure> for Error {
    fn from(failure: Failure) -> Self {
        let code = match &failure {
            Failure::Absent => ErrorCode::NotFound,
            Failure::Retained(_) | Failure::WorkerPanicked(_) => ErrorCode::RecoveryRequired,
            Failure::SystemChanged => ErrorCode::DiskIdentityMismatch,
        };
        storage_contract::Error::new(code, failure.to_string()).into()
    }
}

impl Engine {
    /// Host shutdown/test synchronization; no caller action is advanced here.
    pub fn wait_backup_preparation(&mut self) -> Result<BackupPreparationStatus> {
        if let Some(worker) = self.backup_preparation.as_mut() {
            worker.join()?;
        }
        if let Some(status) = self.backup_preparation_status() {
            // A cold native serving admission can report success or a real
            // blocked handoff without an old history copier.
            return Ok(status);
        }
        let original = self.workerless_backup_request().map_err(|error| {
            // NotFound means proved absence to the shutdown caller. A nested
            // read failure, even one with that code, cannot grant it.
            if error.code() == ErrorCode::NotFound {
                error.context(
                    ErrorCode::RecoveryRequired,
                    "cannot inspect the retained Backup task",
                )
            } else {
                error
            }
        })?;
        match original {
            Some(operation) => Err(Failure::Retained(operation).into()),
            None => Err(Failure::Absent.into()),
        }
    }

    fn workerless_backup_request(&self) -> Result<Option<OperationId>> {
        let directory = self._locks.system_directory_identity();
        let writer = self._locks.system_identity()?;
        let owned_system = || -> Result<DiskRoot> {
            let root = DiskRoot::open(&self.root)?;
            if root.directory_identity(root.path())? != directory {
                return Err(Failure::SystemChanged.into());
            }
            let named = root.open_read_entry(&layout::CONTROL_WRITER_LOCK)?;
            if FileIdentity::from_metadata(&named.metadata()?) != writer {
                return Err(Failure::SystemChanged.into());
            }
            Ok(root)
        };
        let operation = transaction::retained_operation(&owned_system()?)?;
        // A named root/lock replacement during the read is not proof of idle.
        owned_system()?;
        Ok(operation)
    }
}
