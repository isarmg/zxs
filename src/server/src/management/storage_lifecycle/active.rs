//! Shared, original-operation owner for nonterminal deregistration. Directory
//! stop hints never replace the authenticated D3/receipt admission below.
use super::*;
use platform_io::fs_safety::AdvisoryLock;

/// Current primary-credential admission plus one native control owner. The host
/// lifetime lease and Engine ownership belong to the caller. No device erase,
/// mount, permission repair, replacement or force-success operation is provided.
pub struct ActiveExit<'a> {
    backend: &'a EngineBackend,
    identity: &'a Identity,
    system: PathBuf,
    management: PrivateDirectory,
    _owner: AdvisoryLock,
}
impl<'a> ActiveExit<'a> {
    pub fn open(
        backend: &'a EngineBackend,
        identity: &'a Identity,
        management: PrivateDirectory,
    ) -> Result<Self, ApiError> {
        let system = backend.addition_root(identity)?;
        management.verify_identity().map_err(storage_error)?;
        // The same file also serializes native Add owners in this management root.
        let owner = acquire_owner(&management)?;
        Ok(Self {
            backend,
            identity,
            system,
            management,
            _owner: owner,
        })
    }
    fn check(&self) -> Result<(), ApiError> {
        self.management.verify_identity().map_err(storage_error)?;
        if self.backend.addition_root(self.identity)? != self.system {
            return Err(ApiError::conflict(
                ErrorCode::GenerationMismatch,
                "exit owner no longer holds the original SYSTEM authority",
            ));
        }
        Ok(())
    }
    pub fn current_configuration(&self) -> Result<Config, ApiError> {
        self.check()?;
        self.backend.addition_configuration(self.identity)
    }
    /// Authenticated registration intent, not an online/health observation.
    /// Disabled paths are not opened to obtain this projection.
    pub fn registrations(
        &self,
    ) -> Result<storage_contract::storage_readmission::RegistrationSnapshot, ApiError> {
        self.check()?;
        self.backend.storage_registration_snapshot(self.identity)
    }
    fn settled(&self) -> Result<(), ApiError> {
        require_settled(self.management.path())
    }
    /// Preview exact original registration and published boundary. The final
    /// voter is identified as TerminalManagement; this owner's commit rejects it.
    pub fn preview(&self, request: ExitRequest) -> Result<ExitPreview, ApiError> {
        self.check()?;
        deregister(request.action)?;
        self.settled()?;
        Ok(self.backend.preview_storage_exit(self.identity, request)?.0)
    }
    pub fn commit(&self, request: ExitCommit) -> Result<ExitStatus, ApiError> {
        self.check()?;
        admitted_kind(&request.preview)?;
        let directory = self
            .management
            .create_child(&EntryName::new(DIRECTORY).expect("invariant: fixed exit directory"))
            .map_err(storage_error)?;
        let (record, admitted) =
            prepare_record(&directory, &self.identity.principal, request, |request| {
                self.settled()?;
                self.backend.preview_storage_exit(self.identity, request)
            })?;
        self.run_admitted(record, admitted)
    }
    pub fn resume(&self, operation: OperationId) -> Result<ExitStatus, ApiError> {
        self.check()?;
        let directory = open(self.management.path())?.ok_or_else(missing)?;
        let record = read_record(&directory, operation)?.ok_or_else(missing)?;
        self.run_admitted(record, true)
    }
    fn actor(&self, record: &StoredExit) -> Result<(), ApiError> {
        if record.principal != self.identity.principal {
            return Err(ApiError::forbidden());
        }
        admitted_kind(&record.status.preview)
    }
    pub(super) fn run_admitted(
        &self,
        mut record: StoredExit,
        admitted: bool,
    ) -> Result<ExitStatus, ApiError> {
        self.check()?;
        self.actor(&record)?;
        let directory = self
            .management
            .create_child(&EntryName::new(DIRECTORY).expect("invariant: fixed exit directory"))
            .map_err(storage_error)?;
        if let Some(result) = self.backend.storage_exit_result(self.identity, &record)? {
            finish(&mut record, result);
            save(&directory, &record)?;
            return Ok(record.status);
        }
        if record.status.state == ExitState::Completed {
            return Err(ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "saved deregistration completion has no admitted permanent D3 receipt",
            ));
        }
        // The Engine callback first revalidates its source under the Engine
        // mutex, then persists the intent before fencing. No stale-preview
        // record is invented if that admission fails.
        execute_record(
            self.management.path(),
            &directory,
            record,
            admitted,
            |intent, progress| {
                self.backend
                    .run_storage_exit(self.identity, intent, progress)
            },
        )
    }
    pub fn status(&self, operation: OperationId) -> Result<ExitStatus, ApiError> {
        self.check()?;
        let directory = open(self.management.path())?.ok_or_else(missing)?;
        let mut record = read_record(&directory, operation)?.ok_or_else(missing)?;
        self.actor(&record)?;
        if let Some(result) = self.backend.storage_exit_result(self.identity, &record)? {
            finish(&mut record, result);
        } else if record.status.state == ExitState::Completed {
            return Err(ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "cached deregistration completion has no admitted permanent D3 receipt",
            ));
        } else {
            record.status.state = ExitState::RecoveryRequired;
        }
        Ok(record.status)
    }
}
pub(super) fn deregister(action: ExitAction) -> Result<(), ApiError> {
    if action != ExitAction::Deregister {
        return Err(ApiError::invalid(
            "new planned-offline actions are unavailable; deregistration is a distinct explicit action",
        ));
    }
    Ok(())
}
fn admitted_kind(preview: &ExitPreview) -> Result<(), ApiError> {
    deregister(preview.request.action)?;
    if preview.authority != ExitAuthority::ActiveMembership || preview.target_metadata_members == 0
    {
        return Err(ApiError::conflict(
            ErrorCode::RecoveryRequired,
            "final-voter/terminal exit needs its dedicated drain owner and is not supported by active deregistration",
        ));
    }
    Ok(())
}
