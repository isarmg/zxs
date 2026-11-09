//! One user-owned Backup transaction, not a second disk protocol. The retained
//! request fixes source, targets and topology; each action still enters the
//! existing native boundary. Polling never advances or selects anything.
use super::*;

const REQUEST: &str = "backup-preparation/transaction.json";
const MAX_REQUEST: usize = 64 * 1024;
mod cancellation;
pub use cancellation::{BackupTransactionCancelOutcome, BackupTransactionCancellation};
pub(super) use cancellation::{
    cancellation_pending, require_not_cancelling, require_operation_open,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BackupTransactionRequest {
    pub preparation: BackupPreparationRequest,
    pub targets: Vec<BackupTopologyTarget>,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Retained {
    format: String,
    request: BackupTransactionRequest,
    directories: Vec<PhysicalBinding>,
}
impl Retained {
    fn read(root: &DiskRoot) -> Result<Option<Self>> {
        let directory = root.path().join("backup-preparation");
        if root.optional_metadata(&directory)?.is_none() {
            return Ok(None);
        }
        let Some(file) = root.open_optional_read_regular(&root.path().join(REQUEST))? else {
            return Ok(None);
        };
        let saved: Self = storage_contract::decode_strict(&read_file_bytes(file, MAX_REQUEST)?)?;
        saved.request.validate()?;
        if saved.format != "zxs-backup-user-transaction"
            || saved.directories.len() != saved.request.targets.len()
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "invalid retained Backup transaction request",
            ));
        }
        Ok(Some(saved))
    }
    fn require_actor(&self, operation: OperationId, principal: &str) -> Result<()> {
        Engine::require_actor(principal)?;
        if self.request.preparation.operation_id != operation
            || self.request.preparation.principal != principal
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "Backup transaction belongs to another original operation or principal",
            ));
        }
        Ok(())
    }
}
impl BackupTransactionRequest {
    fn validate(&self) -> Result<()> {
        Engine::require_actor(&self.preparation.principal)?;
        let count = self.preparation.target_paths.len();
        if !(1..=8).contains(&count) || count != self.targets.len() {
            return Err(Error::new(
                ErrorCode::ConfigInvalid,
                "Backup transaction requires one to eight complete target directories and exact topology entries",
            ));
        }
        name_policy::validate_slot(&self.preparation.source_slot_id)?;
        let mut disks = std::collections::BTreeSet::new();
        let mut slots = std::collections::BTreeSet::new();
        for target in &self.targets {
            name_policy::validate_slot(&target.slot_id)?;
            name_policy::validate_slot(&target.upstream_slot_id)?;
            if target.disk_id.is_empty()
                || !disks.insert(&target.disk_id)
                || !slots.insert(&target.slot_id)
            {
                return Err(Error::new(
                    ErrorCode::ConfigInvalid,
                    "Backup target identities are empty or duplicated",
                ));
            }
        }
        for (index, path) in self.preparation.target_paths.iter().enumerate() {
            if !path.is_absolute()
                || path.components().any(|part| {
                    !matches!(
                        part,
                        std::path::Component::RootDir | std::path::Component::Normal(_)
                    )
                })
                || self.preparation.target_paths[..index]
                    .iter()
                    .any(|other| path.starts_with(other) || other.starts_with(path))
            {
                return Err(Error::new(
                    ErrorCode::UnsafePath,
                    "Backup target paths are not a disjoint normalized absolute set",
                ));
            }
        }
        Ok(())
    }
}
impl Engine {
    /// Locate the retained original request even if the process stopped before
    /// its first worker was created. This is a read, not an authority to resume
    /// or replace any target. The Engine already owns the SYSTEM writer lock.
    pub fn current_backup_transaction_request(
        &self,
        principal: &str,
    ) -> Result<Option<BackupTransactionRequest>> {
        Self::require_actor(principal)?;
        let Some(saved) = Retained::read(&DiskRoot::open(&self.root)?)? else {
            return Ok(None);
        };
        saved.require_actor(saved.request.preparation.operation_id, principal)?;
        Ok(Some(saved.request))
    }

    /// Read the exact original request without touching a target directory.
    /// A missing task is distinct from corruption or another caller's task.
    pub fn backup_transaction_request(
        &self,
        operation: OperationId,
        principal: &str,
    ) -> Result<Option<BackupTransactionRequest>> {
        Self::require_actor(principal)?;
        let system = DiskRoot::open(&self.root)?;
        if let Some(request) = cancellation::archived_request(&system, operation, principal)? {
            return Ok(Some(request));
        }
        let Some(saved) = Retained::read(&system)? else {
            return Ok(None);
        };
        saved.require_actor(operation, principal)?;
        Ok(Some(saved.request))
    }

    /// Retain one explicit user task before starting its independent full-copy
    /// worker. Descriptor order must match the request. No write stop or native
    /// registry decision is authorized by creating this preparation task.
    pub fn start_backup_transaction(
        &mut self,
        request: BackupTransactionRequest,
        directories: Vec<File>,
    ) -> Result<BackupPreparationStatus> {
        request.validate()?;
        self.require_no_commit_preparation()?;
        let system = DiskRoot::open(&self.root)?;
        require_operation_open(&system, request.preparation.operation_id)?;
        if let Some(saved) = Retained::read(&system)? {
            saved.require_actor(
                request.preparation.operation_id,
                &request.preparation.principal,
            )?;
            if saved.request != request {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "same Backup operation cannot substitute its original source, targets or topology",
                ));
            }
            return self.backup_preparation_status().ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "retained Backup request needs explicit same-operation resume",
                )
            });
        }
        self.require_write()?;
        self.require_unfenced_membership()?;
        self.check_batch_scope(&request.preparation)?;
        self.check_backup_transaction_topology(
            &request.preparation.source_slot_id,
            &request.preparation.target_paths,
            &request.targets,
        )?;
        if self.config.generation().is_some() || self.snapshot.is_none() {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "Backup preparation requires its original fixed-genesis published source",
            ));
        }
        if self
            .state
            .operations
            .contains_key(&request.preparation.operation_id)
            || self.backup_preparation.is_some()
            || crate::history_package::preparation_path_exists(&system, &system.path().join(JOB))?
            || crate::history_package::preparation_path_exists(
                &system,
                &system.path().join(batch::BATCH_JOB),
            )?
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "an existing operation or preparation must retain its original task",
            ));
        }
        if directories.len() != request.targets.len() {
            return Err(Error::new(
                ErrorCode::ConfigInvalid,
                "each Backup target requires its original held directory",
            ));
        }
        let mut bindings = Vec::with_capacity(directories.len());
        let mut roots = Vec::with_capacity(directories.len());
        for (path, directory) in request.preparation.target_paths.iter().zip(directories) {
            let root = DiskRoot::from_directory(path.clone(), directory)?;
            let physical = PhysicalBinding::of(&root)?;
            if PhysicalBinding::of(&DiskRoot::open(path)?)? != physical {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "Backup target name differs from its selected directory",
                ));
            }
            bindings.push(physical);
            if root.entries(root.path())?.next().transpose()?.is_some() {
                return Err(Error::new(
                    ErrorCode::ObjectCollision,
                    "a new Backup transaction requires empty targets; existing bytes are preserved",
                ));
            }
            roots.push(root);
        }
        self.check_batch_targets(&roots)?;
        let admitted = roots
            .iter()
            .map(|root| root.directory(root.path()))
            .collect::<Result<Vec<_>>>()?;
        let saved = Retained {
            format: "zxs-backup-user-transaction".into(),
            request,
            directories: bindings,
        };
        let bytes = canonical_json(&saved)?;
        if bytes.len() > MAX_REQUEST {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "Backup request exceeds 64 KiB",
            ));
        }
        system.private_dir(&system.path().join("backup-preparation"))?;
        crate::history_package::install_preparation_bytes(
            &system,
            &system.path().join(REQUEST),
            &bytes,
        )?;
        self.start_transaction_copy(saved.request.preparation, admitted)
    }

    fn start_transaction_copy(
        &mut self,
        request: BackupPreparationRequest,
        mut directories: Vec<File>,
    ) -> Result<BackupPreparationStatus> {
        if directories.len() == 1 {
            self.start_backup_preparation(
                request,
                directories
                    .pop()
                    .expect("invariant: exactly one selected directory"),
            )
        } else {
            self.start_backup_batch_preparation(request, directories)
        }
    }

    /// Retry only the latest retained action. SYSTEM limits the target set;
    /// each existing native task rechecks its physical bindings and authority.
    /// It cannot turn a failed source into a newly sampled source snapshot.
    pub fn resume_backup_transaction(
        &mut self,
        operation: OperationId,
        principal: &str,
    ) -> Result<BackupPreparationStatus> {
        let system = DiskRoot::open(&self.root)?;
        require_operation_open(&system, operation)?;
        let saved = Retained::read(&system)?
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "no original Backup transaction"))?;
        saved.require_actor(operation, principal)?;
        if self.backup_serving.is_some() {
            // Resume may finish only the activation already selected by this
            // original task. Re-enter the native coordinator so a late failed
            // Current/SYSTEM/config handoff is not mistaken for read-only status.
            self.complete_backup_registry_placement_decision(operation, principal)?;
            return self.transaction_status(operation);
        }
        if self
            .backup_preparation
            .as_ref()
            .and_then(|worker| worker.thread.as_ref())
            .is_some_and(|thread| !thread.is_finished())
        {
            return self.transaction_status(operation);
        }
        if let Some(worker) = self.backup_preparation.as_mut() {
            worker.join()?;
        }
        self.resume_backup_preparation()?;
        if self.backup_preparation.is_some() {
            return self.transaction_status(operation);
        }
        // Death before the first copy task: only reopen the originally selected
        // identities. The ordinary preparation still requires empty targets and
        // the unchanged source sequence before it may write anything there.
        let mut directories = Vec::with_capacity(saved.directories.len());
        for (path, expected) in saved
            .request
            .preparation
            .target_paths
            .iter()
            .zip(&saved.directories)
        {
            let root = DiskRoot::open(path)?;
            if &PhysicalBinding::of(&root)? != expected {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "Backup resume target was replaced",
                ));
            }
            directories.push(root.directory(root.path())?);
        }
        self.start_transaction_copy(saved.request.preparation, directories)
    }

    fn transaction_status(&self, operation: OperationId) -> Result<BackupPreparationStatus> {
        let status = self.backup_preparation_status().ok_or_else(|| {
            Error::new(
                ErrorCode::NotFound,
                "original Backup task has no runtime status",
            )
        })?;
        if status.operation_id != operation {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "Backup status belongs to another operation",
            ));
        }
        Ok(status)
    }

    /// Advance only non-decision preparation. Source fencing, irreversible
    /// registry selection and final serving activation remain separate explicit
    /// APIs. A status read never calls this method.
    pub fn continue_backup_transaction(
        &mut self,
        operation: OperationId,
        principal: &str,
    ) -> Result<BackupPreparationStatus> {
        require_operation_open(&DiskRoot::open(&self.root)?, operation)?;
        let request = self
            .backup_transaction_request(operation, principal)?
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "no original Backup transaction"))?;
        let status = self.transaction_status(operation)?;
        if self
            .backup_preparation
            .as_ref()
            .and_then(|worker| worker.thread.as_ref())
            .is_some_and(|thread| !thread.is_finished())
        {
            return Ok(status);
        }
        match status.phase {
            BackupPreparationPhase::HistoryPrepared => {
                self.start_backup_native_preparation(operation, principal)
            }
            BackupPreparationPhase::NativeControlPrepared => {
                self.start_backup_object_database_preparation(operation, principal)
            }
            BackupPreparationPhase::ObjectDatabasesPrepared => {
                self.start_backup_scoped_database_preparation(operation, principal)
            }
            BackupPreparationPhase::ScopedDatabasesPrepared => self
                .start_backup_cutover_preparation(BackupCutoverPreparationRequest {
                    operation_id: operation,
                    principal: principal.into(),
                    targets: request.targets,
                }),
            BackupPreparationPhase::CutoverInputsPrepared
                if self.backup_cutover_fence_status().is_some() =>
            {
                self.start_backup_target_installation(operation, principal)
            }
            _ => Ok(status),
        }
    }
}
