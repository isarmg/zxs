//! Typed shared active-pool coordinator. HTTP session admission stays in its
//! transport; trusted native callers supply a current primary-auth identity.
use super::*;
use platform_io::fs_safety::AdvisoryLock;

/// Owns one active-pool addition control transaction. This is not a daemon stop
/// lock. Native callers must own the Engine and the host ServiceLease throughout
/// this value's lifetime, and bind its current AuthStore to the EngineBackend.
/// No terminal resume, foreign import, device formatting or deletion is offered.
pub struct ActiveAddition<'a> {
    backend: &'a EngineBackend,
    identity: &'a Identity,
    system: PathBuf,
    management: PrivateDirectory,
    discovery: &'a str,
    _owner: AdvisoryLock,
}
impl<'a> ActiveAddition<'a> {
    /// Open an existing management root. Creates only a persistent coordination
    /// lock file; never opens a replacement empty Engine or credential store.
    pub fn open(
        backend: &'a EngineBackend,
        identity: &'a Identity,
        management: PrivateDirectory,
        discovery: &'a str,
    ) -> Result<Self, ApiError> {
        let system = backend.addition_root(identity)?;
        management.verify_identity().map_err(storage_error)?;
        let owner = AdvisoryLock::acquire(
            &management,
            &EntryName::new("storage-addition-owner.lock")
                .expect("invariant: fixed coordination lock")
                .as_relative(),
        )
        .map_err(storage_error)?;
        Ok(Self {
            backend,
            identity,
            system,
            management,
            discovery,
            _owner: owner,
        })
    }
    /// Current authenticated pool configuration, not a candidate template.
    pub fn current_configuration(&self) -> Result<Config, ApiError> {
        self.check()?;
        self.backend.addition_configuration(self.identity)
    }
    fn check(&self) -> Result<(), ApiError> {
        self.management.verify_identity().map_err(storage_error)?;
        if self.backend.addition_root(self.identity)? != self.system {
            return Err(ApiError::conflict(
                ErrorCode::GenerationMismatch,
                "addition owner no longer holds its original SYSTEM root",
            ));
        }
        Ok(())
    }
    fn existing(&self) -> Result<Option<PrivateDirectory>, ApiError> {
        self.check()?;
        match PrivateDirectory::open_existing(self.management.path().join(DIRECTORY)) {
            Ok(directory) => Ok(Some(directory)),
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(None)
            }
            Err(source) => Err(storage_error(source)),
        }
    }
    fn settled(&self) -> Result<(), ApiError> {
        super::super::storage_lifecycle::require_settled_exit(self.management.path())?;
        super::super::storage_readmission::require_no_pending_readmission(self.management.path())?;
        if let Some(directory) = self.existing()? {
            require_settled_addition(&directory)?;
        }
        Ok(())
    }
    fn prepare(
        &self,
        request: AdditionRequest,
    ) -> Result<
        (
            AdditionPreview,
            Config,
            Vec<storage_engine::PendingMemberDirectory>,
        ),
        ApiError,
    > {
        self.check()?;
        let (config, stopped) = self.backend.addition_directory_scope(self.identity)?;
        prepare_target(
            config,
            AdditionAuthority::ActiveMembership,
            stopped,
            request,
            self.discovery,
            &self.system,
            self.management.path(),
        )
    }
    /// Revalidate empty held directories and target configuration. Preview may
    /// recover existing core metadata while opening the owned Engine; not a
    /// byte-for-byte read-only inspection of an arbitrary foreign disk.
    pub fn preview(&self, request: AdditionRequest) -> Result<AdditionPreview, ApiError> {
        self.settled()?;
        Ok(self.prepare(request)?.0)
    }
    /// Persist the original actor/target/preview before native F/archive/joint/
    /// activation. Only the core's exact permanent receipt reports completion.
    pub fn commit(&self, request: AdditionCommit) -> Result<AdditionStatus, ApiError> {
        self.check()?;
        let directory = self
            .management
            .create_child(&EntryName::new(DIRECTORY).expect("invariant: fixed addition directory"))
            .map_err(storage_error)?;
        let operation = request.preview.request.operation_id;
        let mut held = None;
        let record = if let Some(record) = read_record(&directory, operation)? {
            self.actor(&record)?;
            if record.status.preview != request.preview {
                return Err(conflict(
                    "operation_id already belongs to a different addition",
                ));
            }
            record
        } else {
            self.settled()?;
            let (preview, target, directories) = self.prepare(request.preview.request.clone())?;
            if preview != request.preview {
                return Err(ApiError::conflict(
                    ErrorCode::GenerationMismatch,
                    "addition preview changed; inspect the original pool and all selected roots",
                ));
            }
            let record = StoredAddition {
                source: AdditionSource::ActiveMembership,
                format: Format::Addition,
                principal: self.identity.principal.clone(),
                target,
                archive: None,
                status: AdditionStatus {
                    operation_id: operation,
                    state: AdditionState::Adding,
                    phase: AdditionPhase::Prepared,
                    continuation: AdditionContinuation::Add,
                    result: None,
                    preview,
                    failure: None,
                },
            };
            save(&directory, &record)?;
            held = Some(directories);
            record
        };
        self.run(&directory, record, held)
    }
    /// Continue only the stored choice. Cancellation remains its own original
    /// certified protocol; an Add retry cannot overwrite a saved Cancel choice.
    pub fn resume(&self, operation: OperationId) -> Result<AdditionStatus, ApiError> {
        let directory = self.existing()?.ok_or_else(missing)?;
        let record = read_record(&directory, operation)?.ok_or_else(missing)?;
        self.actor(&record)?;
        self.run(&directory, record, None)
    }
    /// Select an explicitly requested original Add/Cancel continuation. The
    /// transport supplies any action-specific preview confirmation requirement.
    pub fn continue_selected(
        &self,
        operation: OperationId,
        continuation: AdditionContinuation,
        preview_digest: Option<&str>,
    ) -> Result<AdditionStatus, ApiError> {
        let directory = self.existing()?.ok_or_else(missing)?;
        let mut record = read_record(&directory, operation)?.ok_or_else(missing)?;
        self.actor(&record)?;
        if preview_digest.is_some_and(|digest| digest != record.status.preview.preview_digest) {
            return Err(conflict(
                "cancellation does not bind the original addition preview",
            ));
        }
        if !record.status.state.is_terminal() {
            record.status.continuation = continuation;
        }
        self.run(&directory, record, None)
    }
    fn actor(&self, record: &StoredAddition) -> Result<(), ApiError> {
        if record.principal != self.identity.principal {
            return Err(ApiError::forbidden());
        }
        if record.status.preview.authority != AdditionAuthority::ActiveMembership {
            return Err(ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "this operation belongs to terminal metadata resume, not an active-pool addition",
            ));
        }
        Ok(())
    }
    fn run(
        &self,
        directory: &PrivateDirectory,
        record: StoredAddition,
        held: Option<Vec<storage_engine::PendingMemberDirectory>>,
    ) -> Result<AdditionStatus, ApiError> {
        self.check()?;
        self.actor(&record)?;
        if record.status.state.is_terminal() {
            return self.status(record.status.operation_id);
        }
        super::super::storage_lifecycle::require_settled_exit(self.management.path())?;
        super::super::storage_readmission::require_no_pending_readmission(self.management.path())?;
        execute_active(self.backend, self.identity, directory, record, held)
    }
    /// Read the original intent and reconcile with the exact core receipt. This
    /// query does not retry publication or treat a saved running phase as success.
    pub fn status(&self, operation: OperationId) -> Result<AdditionStatus, ApiError> {
        let directory = self.existing()?.ok_or_else(missing)?;
        let mut record = read_record(&directory, operation)?.ok_or_else(missing)?;
        self.actor(&record)?;
        if let Some(result) = self
            .backend
            .storage_addition_result(self.identity, &record)?
        {
            finish(&mut record, result);
        } else if !record.status.state.is_terminal() {
            record.status.state = AdditionState::RecoveryRequired;
        } else {
            return Err(ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "saved addition completion has no matching permanent core receipt",
            ));
        }
        Ok(record.status)
    }
}
fn missing() -> ApiError {
    ApiError::new(
        http::StatusCode::NOT_FOUND,
        ErrorCode::OperationNotFound,
        "the original active-pool addition was not found",
    )
}
