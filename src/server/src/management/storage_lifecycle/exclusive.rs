//! Unique native owner for an active-to-terminal handover. There are no service
//! threads or borrowed business handles: core still proves actual descriptor
//! closure, and only the admitted terminal authority replaces the Engine.
use super::*;
use crate::auth::AuthReader;
use storage_contract::storage_readmission::RegistrationSnapshot;
use storage_engine::{Engine, TerminalManagement};

enum Source {
    Active(Box<EngineBackend>),
    Terminal(Box<TerminalManagement>),
}

/// Admitted, immutable terminal source metadata. This is a comparison boundary,
/// not a successor authority, permission to write, or a replacement for opening
/// and revalidating the original native owner.
pub struct TerminalBoundary {
    pub operation_id: OperationId,
    pub authority_hash: String,
    pub registry: storage_engine::DiskRegistryCommitment,
    pub sequence: u64,
    pub topology_generation: u64,
    pub pending_retirement: Option<TerminalRetirementPending>,
}

/// Owns an existing SYSTEM and its exclusive Engine or terminal authority.
/// The caller retains the host lifetime lease and the live primary AuthStore.
/// This is not first initialization, a service runtime, or foreign admission.
/// No Engine reference/clone is exposed, so a terminal commit has a unique owner.
pub struct ExclusiveExit {
    source: Source,
    system: PrivateDirectory,
    management: PrivateDirectory,
    auth: AuthReader,
    identity: Identity,
}
impl ExclusiveExit {
    /// Select protected terminal evidence before attempting Engine::open.
    /// Missing/corrupt/pending metadata-resume authority fails without fallback.
    /// Existing native recovery may synchronize its original evidence here.
    pub fn open(
        system: PrivateDirectory,
        management: PrivateDirectory,
        auth: AuthReader,
        identity: Identity,
    ) -> Result<Self, ApiError> {
        auth.revalidate_identity(&identity)?;
        if identity.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        system.verify_identity().map_err(storage_error)?;
        let _owner = acquire_owner(&management)?;
        let source = if TerminalManagement::requires_management(system.path())
            .map_err(crate::engine_backend::error)?
        {
            Source::Terminal(Box::new(
                TerminalManagement::open(system.path()).map_err(crate::engine_backend::error)?,
            ))
        } else {
            let engine = Engine::open(system.path()).map_err(crate::engine_backend::error)?;
            let backend = EngineBackend::new(engine);
            backend.set_management_auth(auth.clone())?;
            Source::Active(Box::new(backend))
        };
        let owner = Self {
            source,
            system,
            management,
            auth,
            identity,
        };
        owner.check()?;
        Ok(owner)
    }
    fn check(&self) -> Result<(), ApiError> {
        self.auth.revalidate_identity(&self.identity)?;
        self.system.verify_identity().map_err(storage_error)?;
        self.management.verify_identity().map_err(storage_error)?;
        if let Source::Active(backend) = &self.source
            && backend.addition_root(&self.identity)? != self.system.path()
        {
            return Err(ApiError::conflict(
                ErrorCode::GenerationMismatch,
                "exclusive exit owner no longer holds the original SYSTEM root",
            ));
        }
        Ok(())
    }
    /// Capture only the held SYSTEM authority and registry commitment. No
    /// stopped or retained data root is probed, and no successor is prepared.
    pub fn terminal_boundary(&self) -> Result<Option<TerminalBoundary>, ApiError> {
        self.check()?;
        let Source::Terminal(terminal) = &self.source else {
            return Ok(None);
        };
        let status = terminal.status();
        Ok(Some(TerminalBoundary {
            operation_id: status.operation_id,
            authority_hash: status.authority_hash,
            registry: terminal.registry().commitment().clone(),
            sequence: terminal.state().sequence,
            topology_generation: terminal.state().topology_generation,
            pending_retirement: terminal
                .terminal_retirement_status()
                .map_err(crate::engine_backend::error)?
                .map(retirement_pending),
        }))
    }
    pub fn current_configuration(&self) -> Result<Config, ApiError> {
        self.check()?;
        match &self.source {
            Source::Active(backend) => backend.addition_configuration(&self.identity),
            Source::Terminal(terminal) => Ok((**terminal.original_configuration()).clone()),
        }
    }
    /// Historical registration intent, never availability or disk health.
    /// Terminal projections do not resolve any retired target pathname.
    pub fn registrations(&self) -> Result<RegistrationSnapshot, ApiError> {
        self.check()?;
        match &self.source {
            Source::Active(backend) => backend.storage_registration_snapshot(&self.identity),
            Source::Terminal(terminal) => {
                super::super::storage_readmission::terminal_registration_snapshot(terminal)
            }
        }
    }
    pub fn preview(&self, request: ExitRequest) -> Result<ExitPreview, ApiError> {
        self.check()?;
        active::deregister(request.action)?;
        match &self.source {
            Source::Active(backend) => ActiveExit::open(
                backend,
                &self.identity,
                self.management.try_clone().map_err(storage_error)?,
            )?
            .preview(request),
            Source::Terminal(_) => Err(terminal_mutation_unavailable()),
        }
    }
    /// Final-voter commit consumes the uniquely held Engine only after the core
    /// closes its target I/O and publishes authenticated terminal authority.
    /// Failure retains its original Engine/drain handle for same-owner retry.
    pub fn commit(&mut self, request: ExitCommit) -> Result<ExitStatus, ApiError> {
        self.check()?;
        active::deregister(request.preview.request.action)?;
        if request.preview.authority == ExitAuthority::ActiveMembership
            && let Source::Active(backend) = &self.source
        {
            return ActiveExit::open(
                backend,
                &self.identity,
                self.management.try_clone().map_err(storage_error)?,
            )?
            .commit(request);
        }
        let _owner = acquire_owner(&self.management)?;
        let directory = self
            .management
            .create_child(&EntryName::new(DIRECTORY).expect("invariant: fixed exit directory"))
            .map_err(storage_error)?;
        let (mut record, admitted) =
            prepare_record(&directory, &self.identity.principal, request, |request| {
                require_settled(self.management.path())?;
                match &self.source {
                    Source::Active(backend) => {
                        backend.preview_storage_exit(&self.identity, request)
                    }
                    Source::Terminal(_) => Err(terminal_mutation_unavailable()),
                }
            })?;
        if let Some(result) = self.receipt(&mut record)? {
            finish(&mut record, result);
            save(&directory, &record)?;
            return Ok(record.status);
        }
        if record.status.state == ExitState::Completed {
            return Err(no_receipt());
        }
        if record.status.preview.authority != ExitAuthority::TerminalManagement
            || record.status.preview.target_metadata_members != 0
        {
            return Err(terminal_mutation_unavailable());
        }
        let status = execute_record(
            self.management.path(),
            &directory,
            record,
            admitted,
            |intent, progress| {
                self.auth.revalidate_identity(&self.identity)?;
                let Source::Active(backend) = &mut self.source else {
                    return Err(terminal_mutation_unavailable());
                };
                // Same unique-backend completion used by the service after its
                // Arc drain. It revalidates the fence before the first callback.
                let terminal = backend.complete_terminal_exit(&self.identity, intent, progress)?;
                let result = terminal.result().clone();
                self.source = Source::Terminal(Box::new(terminal));
                Ok(result)
            },
        )?;
        self.check()?;
        Ok(status)
    }
    fn receipt(&self, record: &mut StoredExit) -> Result<Option<OperationResult>, ApiError> {
        self.check()?;
        if record.principal != self.identity.principal {
            return Err(ApiError::forbidden());
        }
        active::deregister(record.status.preview.request.action)?;
        match &self.source {
            Source::Active(backend) => backend.storage_exit_result(&self.identity, record),
            Source::Terminal(terminal) => super::super::terminal_backend::terminal_exit_result(
                terminal,
                &self.identity,
                record,
            ),
        }
    }
    /// Exact permanent receipt or RecoveryRequired. A cached Completed row is
    /// insufficient, including after a cold terminal reopen or target removal.
    pub fn status(&self, operation: OperationId) -> Result<ExitStatus, ApiError> {
        self.check()?;
        let _owner = acquire_owner(&self.management)?;
        let directory = open(self.management.path())?.ok_or_else(missing)?;
        let mut record = read_record(&directory, operation)?.ok_or_else(missing)?;
        if let Some(result) = self.receipt(&mut record)? {
            finish(&mut record, result);
        } else if record.status.state == ExitState::Completed {
            return Err(no_receipt());
        } else {
            record.status.state = ExitState::RecoveryRequired;
        }
        Ok(record.status)
    }
}
fn terminal_mutation_unavailable() -> ApiError {
    ApiError::conflict(
        ErrorCode::RecoveryRequired,
        "terminal authority permits original receipt recovery, not a new registry mutation or implicit metadata restart",
    )
}
fn no_receipt() -> ApiError {
    ApiError::conflict(
        ErrorCode::RecoveryRequired,
        "saved exit completion has no admitted exact permanent publication receipt",
    )
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/management/exclusive_exit.rs"]
mod tests;
