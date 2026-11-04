//! Exclusive handover from a draining Engine to authenticated system-only state.
use super::*;
use storage_contract::{
    ErrorCode, OperationResult,
    storage_lifecycle::{ExitPhase, TerminalRegistrations, TerminalStorageStatus},
};
use storage_engine::TerminalManagement;

pub(super) enum BackendState {
    Unopened,
    Engine(Arc<EngineBackend>),
    /// Current owners may finish; no new business or background work is admitted.
    Draining(Arc<EngineBackend>),
    /// The lifecycle coordinator owns the unique Engine outside the state mutex.
    ChangingAuthority,
    Terminal(Arc<TerminalManagement>),
    MetadataResume(Box<storage_engine::TerminalMetadataResume>),
    /// Core consumed a handle or startup found its immutable pending selector.
    /// Only the original authenticated operation may restore the selected state.
    ResumeRecovery,
}
impl BackendState {
    pub(super) fn active_engine(&self) -> Option<&Arc<EngineBackend>> {
        match self {
            Self::Engine(backend) => Some(backend),
            Self::Unopened
            | Self::Draining(_)
            | Self::ChangingAuthority
            | Self::Terminal(_)
            | Self::MetadataResume(_)
            | Self::ResumeRecovery => None,
        }
    }
    pub(super) fn engine(&self) -> Option<&Arc<EngineBackend>> {
        match self {
            Self::Engine(backend) | Self::Draining(backend) => Some(backend),
            Self::Unopened
            | Self::ChangingAuthority
            | Self::Terminal(_)
            | Self::MetadataResume(_)
            | Self::ResumeRecovery => None,
        }
    }
    pub(super) fn terminal(&self) -> Option<&Arc<TerminalManagement>> {
        match self {
            Self::Terminal(terminal) => Some(terminal),
            Self::Unopened
            | Self::Engine(_)
            | Self::Draining(_)
            | Self::ChangingAuthority
            | Self::MetadataResume(_)
            | Self::ResumeRecovery => None,
        }
    }
}
impl Inner {
    pub(super) fn current_management_identity(
        &self,
        caller: &Identity,
    ) -> Result<Identity, ApiError> {
        self.ensure_running()?;
        if caller.admission_session.is_none() {
            return Err(ApiError::unauthorized());
        }
        let auth = crate::lock(&self.auth)
            .clone()
            .ok_or_else(ApiError::unauthorized)?;
        crate::lock(&auth).revalidate_identity(caller)
    }

    /// Runs before setup proof and Engine::open, neither of which may resolve a
    /// stopped root. A bad terminal head stays an error, never first activation.
    pub(super) fn recover_terminal(&self) -> Result<bool, ApiError> {
        {
            let state = crate::lock(&self.state);
            if matches!(
                state.backend,
                BackendState::Draining(_)
                    | BackendState::ChangingAuthority
                    | BackendState::Terminal(_)
                    | BackendState::MetadataResume(_)
                    | BackendState::ResumeRecovery
            ) {
                return Ok(true);
            }
            if state.backend.engine().is_some() {
                return Ok(false);
            }
        }
        if !TerminalManagement::requires_management(&self.data_root)
            .map_err(crate::engine_backend::error)?
        {
            return Ok(false);
        }
        if TerminalManagement::metadata_resume_recovery(&self.data_root)
            .map_err(crate::engine_backend::error)?
            .is_some()
        {
            let mut state = crate::lock(&self.state);
            state.backend = BackendState::ResumeRecovery;
            state.admitted = false;
            state.status = StorageState::RecoveryRequired;
            return Ok(true);
        }
        let terminal =
            TerminalManagement::open(&self.data_root).map_err(crate::engine_backend::error)?;
        self.install_terminal(terminal);
        Ok(true)
    }

    fn install_terminal(&self, terminal: TerminalManagement) {
        let mut state = crate::lock(&self.state);
        state.backend = BackendState::Terminal(Arc::new(terminal));
        state.admitted = false;
        state.status = if self.stopping.load(Ordering::SeqCst) {
            StorageState::Unavailable
        } else {
            StorageState::TerminalManagement
        };
        state.observed_at = std::time::Instant::now();
        drop(state);
        *crate::lock(&self.capabilities) = crate::Capabilities::unavailable();
        crate::lock(&self.disk_observation).take();
    }

    pub(super) fn terminal(
        &self,
        identity: &Identity,
    ) -> Result<Option<Arc<TerminalManagement>>, ApiError> {
        let terminal = crate::lock(&self.state).backend.terminal().cloned();
        if terminal.is_some() {
            self.current_management_identity(identity)?;
        }
        Ok(terminal)
    }

    pub(super) fn terminal_storage_status(
        &self,
        identity: &Identity,
    ) -> Result<Option<crate::setup_recovery::StorageActivationStatus>, ApiError> {
        let Some(terminal) = self.terminal(identity)? else {
            return Ok(None);
        };
        let status = terminal.status();
        Ok(Some(crate::setup_recovery::StorageActivationStatus {
            state: crate::setup_recovery::StorageActivationState::TerminalManagement,
            disk_ids: terminal
                .registered_disks()
                .map(|disk| disk.disk_id.to_owned())
                .collect(),
            completion_available: false,
            allow_development: self.allow_development_setup,
            terminal: Some(TerminalStorageStatus {
                pool_id: status.target.pool_id,
                operation_id: status.operation_id,
                authority_hash: status.authority_hash,
                registrations: match status.registrations {
                    storage_engine::TerminalRegistrationState::RetainedRegistrations => {
                        TerminalRegistrations::RetainedRegistrations
                    }
                    storage_engine::TerminalRegistrationState::EmptyRegistry => {
                        TerminalRegistrations::EmptyRegistry
                    }
                },
                registered_directories: status.registered_disks,
                original_metadata_members: status.original_metadata_members,
                original_metadata_quorum: status.original_metadata_quorum,
            }),
        }))
    }

    pub(super) fn terminal_disk_status(
        &self,
        identity: &Identity,
    ) -> Result<Option<Vec<storage_contract::DiskStatus>>, ApiError> {
        self.terminal(identity)
            .map(|terminal| terminal.map(|terminal| terminal.disk_status()))
    }

    pub(super) fn terminal_operation(
        &self,
        identity: &Identity,
        operation: storage_contract::OperationId,
    ) -> Result<Option<storage_contract::OperationOutcome>, ApiError> {
        let Some(terminal) = self.terminal(identity)? else {
            return Ok(None);
        };
        let result = terminal.operation(operation).ok_or_else(|| {
            ApiError::new(
                http::StatusCode::NOT_FOUND,
                ErrorCode::OperationNotFound,
                "operation was not found in authenticated terminal history",
            )
        })?;
        if terminal
            .state()
            .operation_principals
            .get(&operation)
            .map(String::as_str)
            != Some(identity.principal.as_str())
        {
            return Err(ApiError::forbidden());
        }
        if let Some(history) = &result.history_tree_id {
            crate::engine_backend::repo_for_history_in_state(terminal.state(), history)?;
        }
        Ok(Some(result.clone().into()))
    }

    pub(super) fn terminal_exit_result(
        &self,
        identity: &Identity,
        intent: &mut storage_lifecycle::StoredExit,
    ) -> Result<Option<OperationResult>, ApiError> {
        let Some(terminal) = self.terminal(identity)? else {
            return Ok(None);
        };
        let operation = intent.status.operation_id;
        if terminal.original_configuration().pool_id != intent.status.preview.target.pool_id {
            return Err(ApiError::conflict(
                ErrorCode::RecoveryRequired,
                "terminal receipt belongs to another pool",
            ));
        }
        if terminal.status().operation_id == operation {
            let archive = terminal.control_archive();
            let binding = storage_addition::ArchiveBinding {
                archive_path: archive.archive_path,
                root_hash: archive.root_hash,
                transition_hash: archive.transition_hash,
            };
            if intent
                .archive
                .as_ref()
                .is_some_and(|saved| saved != &binding)
            {
                return Err(ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "terminal receipt differs from the saved control archive",
                ));
            }
            intent.archive = Some(binding);
        }
        let Some(result) = terminal.operation(operation) else {
            return Ok(None);
        };
        if terminal
            .state()
            .operation_principals
            .get(&operation)
            .map(String::as_str)
            != Some(identity.principal.as_str())
        {
            return Err(ApiError::forbidden());
        }
        crate::engine_backend::storage_lifecycle::exact_result(
            result.clone(),
            intent,
            intent.archive.as_ref(),
        )
        .map(Some)
    }

    pub(super) fn run_terminal_exit(
        &self,
        identity: &Identity,
        intent: &storage_lifecycle::StoredExit,
        mut progress: impl FnMut(
            ExitPhase,
            Option<storage_addition::ArchiveBinding>,
        ) -> Result<(), ApiError>,
    ) -> Result<OperationResult, ApiError> {
        self.current_management_identity(identity)?;
        let owner = {
            let mut state = crate::lock(&self.state);
            let backend = state.backend.engine().cloned().ok_or_else(|| {
                ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "the original terminal exit engine is unavailable",
                )
            })?;
            state.backend = BackendState::Draining(Arc::clone(&backend));
            state.admitted = false;
            state.status = StorageState::RecoveryRequired;
            backend
        };
        // Durable fence precedes waiting for Arc owners. An already-admitted
        // caller queued on the engine mutex then sees the actual core fence.
        if let Err(error) = owner.fence_terminal_exit(identity, intent, &mut progress) {
            if !owner.has_membership_fence() {
                // An unfenced stale preview must not create an unrecoverable
                // draining state. Keep admission closed until normal recovery.
                crate::lock(&self.state).backend = BackendState::Engine(owner);
            }
            return Err(error);
        }
        drop(owner);
        let mut backend = {
            let mut state = crate::lock(&self.state);
            state.admitted = false;
            state.status = StorageState::RecoveryRequired;
            let previous = std::mem::replace(&mut state.backend, BackendState::ChangingAuthority);
            let backend = match previous {
                BackendState::Engine(backend) | BackendState::Draining(backend) => backend,
                other => {
                    state.backend = other;
                    return Err(ApiError::conflict(
                        ErrorCode::RecoveryRequired,
                        "the original engine is not available for terminal exit",
                    ));
                }
            };
            match Arc::try_unwrap(backend) {
                Ok(backend) => backend,
                Err(backend) => {
                    state.backend = BackendState::Draining(backend);
                    let mut error = ApiError::conflict(
                        ErrorCode::StorageDraining,
                        "existing operations must release the original engine before terminal handover; continue the same operation",
                    );
                    error.retryable = true;
                    return Err(error);
                }
            }
        };
        match backend.complete_terminal_exit(identity, intent, progress) {
            Ok(terminal) => {
                // Core complete verified actual descriptor closure. Unique Rust
                // ownership additionally releases every old Engine-only writer.
                drop(backend);
                let result = terminal.result().clone();
                self.install_terminal(terminal);
                Ok(result)
            }
            Err(error) => {
                // Keep the same handle and Engine after a failed drain or sync.
                crate::lock(&self.state).backend = BackendState::Draining(Arc::new(backend));
                Err(error)
            }
        }
    }
}
impl ManagementBackend {
    pub(super) fn update_terminal_disk_note(
        &self,
        identity: &Identity,
        terminal: &TerminalManagement,
        slot: &str,
        request: storage_contract::disk_details::DiskNoteRequest,
    ) -> Result<storage_contract::disk_details::DiskNoteResponse, ApiError> {
        let _notes = crate::lock(&self.inner.disk_notes);
        self.inner.current_management_identity(identity)?;
        let disk = terminal
            .registered_disks()
            .find(|disk| disk.slot_id.eq_ignore_ascii_case(slot))
            .ok_or_else(|| {
                ApiError::new(
                    http::StatusCode::NOT_FOUND,
                    ErrorCode::NotFound,
                    "registered directory was not found",
                )
            })?;
        if request.expected_disk_id != disk.disk_id {
            return Err(ApiError::conflict(
                ErrorCode::DiskIdentityMismatch,
                "registered disk changed; reload before updating its note",
            ));
        }
        let note = disk_notes::update(
            &self.inner.management_root,
            disk.disk_id,
            &request.expected_note,
            request.note,
        )?;
        Ok(storage_contract::disk_details::DiskNoteResponse {
            slot_id: disk.slot_id.to_owned(),
            disk_id: disk.disk_id.to_owned(),
            note,
        })
    }

    pub(super) fn terminal_disk_details(
        &self,
        identity: &Identity,
        slot: &str,
    ) -> Result<Option<storage_contract::disk_details::DiskDetailsRecord>, ApiError> {
        let Some(terminal) = self.inner.terminal(identity)? else {
            return Ok(None);
        };
        let registered = terminal
            .registered_disks()
            .find(|disk| disk.slot_id.eq_ignore_ascii_case(slot))
            .ok_or_else(|| {
                ApiError::new(
                    http::StatusCode::NOT_FOUND,
                    ErrorCode::NotFound,
                    "registered directory was not found",
                )
            })?;
        let disk = terminal
            .original_configuration()
            .disks
            .iter()
            .find(|disk| disk.disk_id == registered.disk_id)
            .expect("invariant: terminal registration retains its historical directory");
        let root_path = disk
            .root
            .to_str()
            .ok_or_else(|| ApiError::invalid("registered directory is not a UTF-8 path"))?
            .to_owned();
        let note = {
            let _notes = crate::lock(&self.inner.disk_notes);
            disk_notes::read(&self.inner.management_root, &disk.disk_id)?
        };
        use storage_contract::disk_details::*;
        Ok(Some(DiskDetailsRecord {
            slot_id: registered.slot_id.to_owned(),
            disk_id: registered.disk_id.to_owned(),
            root_path,
            mount_path: None,
            model: None,
            serial: None,
            capacity_bytes: None,
            device_path: None,
            filesystem_uuid: None,
            mapping: DiskMapping::Unknown,
            health: DiskHealth::Unavailable {
                reason: DiskHealthUnavailable::MappingUnknown,
            },
            note,
        }))
    }
}
