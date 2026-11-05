//! Existing Add endpoints, backed by the authenticated terminal authority.
//! HTTP intent is only retry context; real new-Q publication is core authority.
use super::*;
mod cancel;
use platform_io::fs_safety::PrivateDirectory;
use storage_addition::{
    AdditionSource, ArchiveBinding, StoredAddition, configuration_digest, finish, latest_name,
    read, save, write,
};
use storage_contract::{Config, ErrorCode, OperationId, OperationResult, storage_addition::*};
use storage_engine::{
    PendingMemberDirectory, TerminalManagement, TerminalMetadataResume,
    TerminalMetadataResumeAdmission, TerminalMetadataResumeCancellationPhase,
    TerminalMetadataResumePhase, TerminalMetadataResumeRecovery,
};

fn recovery(message: &'static str) -> ApiError {
    ApiError::conflict(ErrorCode::RecoveryRequired, message)
}

pub(crate) fn exact_result(
    result: OperationResult,
    intent: &StoredAddition,
) -> Result<OperationResult, ApiError> {
    let AdditionAuthority::TerminalManagement {
        terminal_authority_hash,
        ..
    } = &intent.status.preview.authority
    else {
        return Err(recovery(
            "metadata resume receipt requires its terminal authority",
        ));
    };
    if result.operation_id != intent.status.operation_id
        || !matches!(
            result.kind.as_str(),
            "terminal-metadata-resume" | "terminal-metadata-resume-cancelled"
        )
        || result.commit_state != storage_contract::CommitState::Published
        || intent
            .archive
            .as_ref()
            .is_none_or(|binding| binding.transition_hash != result.request_digest)
        || result
            .result
            .get("terminal_authority")
            .and_then(Value::as_str)
            != Some(terminal_authority_hash.as_str())
        || result
            .result
            .get("metadata_members")
            .and_then(Value::as_u64)
            != Some(intent.status.preview.target_metadata_members as u64)
    {
        return Err(recovery(
            "publication did not confirm the original terminal metadata resume",
        ));
    }
    Ok(result)
}

/// No I/O: reject another actor, target or terminal source before
/// taking the exclusive live owner out of the dispatcher.
fn require_resume_owner(
    pending: &TerminalMetadataResume,
    intent: &StoredAddition,
) -> Result<(), ApiError> {
    let terminal = pending.terminal_management();
    let status = terminal.status();
    let expected = AdditionAuthority::TerminalManagement {
        terminal_authority_hash: status.authority_hash,
        terminal_operation_id: status.operation_id,
    };
    if pending.admission() != TerminalMetadataResumeAdmission::FreshDirectories
        || pending.operation_id() != intent.status.operation_id
        || pending.principal() != intent.principal
        || expected != intent.status.preview.authority
        || configuration_digest(terminal.original_configuration())
            != intent.status.preview.configuration_digest
        || configuration_digest(pending.target_configuration())
            != configuration_digest(&intent.target)
    {
        return Err(ApiError::conflict(
            ErrorCode::IdempotencyKeyReused,
            "fresh resume does not own this exact actor, operation, source and target",
        ));
    }
    Ok(())
}

fn bind_held_resume(
    pending: &TerminalMetadataResume,
    intent: &mut StoredAddition,
) -> Result<(), ApiError> {
    require_resume_owner(pending, intent)?;
    let status = pending.status().map_err(crate::engine_backend::error)?;
    let archive = pending.control_archive();
    let binding = ArchiveBinding {
        archive_path: archive.archive_path,
        root_hash: archive.root_hash,
        transition_hash: status.transition_hash,
    };
    let AdditionAuthority::TerminalManagement {
        terminal_authority_hash,
        ..
    } = &intent.status.preview.authority
    else {
        return Err(recovery(
            "the held resume has no terminal authority binding",
        ));
    };
    if status.admission != TerminalMetadataResumeAdmission::FreshDirectories
        || pending.principal() != intent.principal
        || status.operation_id != intent.status.operation_id
        || &status.terminal_authority_hash != terminal_authority_hash
        || status.original_metadata_members != intent.status.preview.original_metadata_members
        || status.target_metadata_members != intent.status.preview.target_metadata_members
        || intent
            .archive
            .as_ref()
            .is_some_and(|saved| saved != &binding)
    {
        return Err(recovery(
            "the held resume belongs to another exact operation",
        ));
    }
    intent.archive = Some(binding);
    Ok(())
}

enum ResumeOwner {
    Pending(Box<TerminalMetadataResume>),
    Published(Box<OperationResult>),
}

impl Inner {
    fn with_resume_authorization<T>(
        &self,
        identity: &Identity,
        action: impl FnOnce() -> Result<T, storage_engine::Error>,
    ) -> Result<T, ApiError> {
        self.ensure_running()?;
        let auth = crate::lock(&self.auth)
            .clone()
            .ok_or_else(ApiError::unauthorized)?;
        let current = crate::lock(&auth);
        if identity.admission_session.is_none() {
            return Err(ApiError::unauthorized());
        }
        current.revalidate_identity(identity)?;
        // Serialize revocation with the core's publication call. A session
        // checked before copying the archive cannot authorize a later commit.
        let result = action().map_err(crate::engine_backend::error);
        drop(current);
        result
    }

    pub(super) fn terminal_configuration(
        &self,
        identity: &Identity,
    ) -> Result<(Config, AdditionAuthority), ApiError> {
        let terminal = self.terminal(identity)?.ok_or_else(|| {
            recovery("continue the pending operation before selecting a new metadata group")
        })?;
        let status = terminal.status();
        Ok((
            (**terminal.original_configuration()).clone(),
            AdditionAuthority::TerminalManagement {
                terminal_authority_hash: status.authority_hash,
                terminal_operation_id: status.operation_id,
            },
        ))
    }

    pub(super) fn terminal_resume_result(
        &self,
        identity: &Identity,
        intent: &StoredAddition,
    ) -> Result<Option<OperationResult>, ApiError> {
        self.current_management_identity(identity)?;
        // Only an admitted owner supplies permanent results. A cancellation
        // certificate merely readable after a failed fsync is not completion;
        // explicit recovery opens TerminalManagement and repeats its barriers.
        if let Some(terminal) = self.terminal(identity)? {
            if let Some(result) = terminal.operation(intent.status.operation_id) {
                if terminal
                    .state()
                    .operation_principals
                    .get(&intent.status.operation_id)
                    .map(String::as_str)
                    != Some(identity.principal.as_str())
                {
                    return Err(ApiError::forbidden());
                }
                return exact_result(result.clone(), intent).map(Some);
            }
            return Ok(None);
        }
        let backend = {
            let state = crate::lock(&self.state);
            state
                .backend
                .active_engine()
                .filter(|_| state.admitted)
                .cloned()
        };
        match backend {
            Some(backend) => backend.storage_addition_result(identity, intent),
            None => Ok(None),
        }
    }

    /// Read only the exact already-held pending owner. A cold HTTP progress
    /// record cannot establish whether a real new quorum has chosen Resume.
    pub(super) fn terminal_resume_cancellation_phase(
        &self,
        intent: &StoredAddition,
    ) -> Result<AdditionPhase, ApiError> {
        let state = crate::lock(&self.state);
        let BackendState::MetadataResume(pending) = &state.backend else {
            return Ok(AdditionPhase::ResumeCancellationUnknown);
        };
        require_resume_owner(pending, intent)?;
        let observed = pending
            .cancellation_phase()
            .map_err(crate::engine_backend::error)?;
        Ok(match observed {
            TerminalMetadataResumeCancellationPhase::Cancellable => {
                AdditionPhase::ResumeCancellable
            }
            TerminalMetadataResumeCancellationPhase::Cancelling => AdditionPhase::CertifyingAbort,
            TerminalMetadataResumeCancellationPhase::ResumeQuorumAccepted
            | TerminalMetadataResumeCancellationPhase::GenerationSelected => {
                AdditionPhase::ResumeQuorumAccepted
            }
            // This still needs explicit durable handover; it is not a completed
            // HTTP operation and may only offer continuation of cancellation.
            TerminalMetadataResumeCancellationPhase::Cancelled => AdditionPhase::Cancelling,
            TerminalMetadataResumeCancellationPhase::RetainedReadmission => {
                return Err(recovery(
                    "fresh cancellation cannot own retained readmission",
                ));
            }
        })
    }

    fn restore_resume(&self, pending: TerminalMetadataResume) {
        let mut state = crate::lock(&self.state);
        state.backend = BackendState::MetadataResume(Box::new(pending));
        state.admitted = false;
        state.status = StorageState::RecoveryRequired;
    }

    fn bind_resume_recovery(
        &self,
        proof: &TerminalMetadataResumeRecovery,
        intent: &mut StoredAddition,
    ) -> Result<(), ApiError> {
        let AdditionAuthority::TerminalManagement {
            terminal_authority_hash,
            terminal_operation_id,
        } = &intent.status.preview.authority
        else {
            return Err(recovery(
                "resume proof requires the original terminal preview",
            ));
        };
        if proof.admission != TerminalMetadataResumeAdmission::FreshDirectories
            || &proof.terminal_operation_id != terminal_operation_id
            || configuration_digest(&proof.original) != intent.status.preview.configuration_digest
            || proof.operation_id != intent.status.operation_id
            || proof.principal != intent.principal
            || configuration_digest(&proof.target) != configuration_digest(&intent.target)
            || &proof.terminal_authority_hash != terminal_authority_hash
        {
            return Err(ApiError::conflict(
                ErrorCode::IdempotencyKeyReused,
                "protected metadata resume differs from this operation, actor or target",
            ));
        }
        let binding = ArchiveBinding {
            archive_path: proof.control_archive.archive_path.clone(),
            root_hash: proof.control_archive.root_hash.clone(),
            transition_hash: proof.transition_hash.clone(),
        };
        if intent
            .archive
            .as_ref()
            .is_some_and(|saved| saved != &binding)
        {
            return Err(recovery(
                "resume archive differs from the original saved request",
            ));
        }
        intent.archive = Some(binding);
        Ok(())
    }

    fn open_resume_directories(
        &self,
        intent: &StoredAddition,
    ) -> Result<Vec<PendingMemberDirectory>, ApiError> {
        let AdditionSource::TerminalManagement { original } = &intent.source else {
            return Err(recovery(
                "resume has no authenticated historical configuration",
            ));
        };
        let stopped: Vec<_> = original
            .disks
            .iter()
            .map(|disk| disk.root.clone())
            .collect();
        intent
            .status
            .preview
            .request
            .additions
            .iter()
            .map(|addition| {
                let disk = intent
                    .target
                    .disks
                    .iter()
                    .find(|disk| disk.slot_id == addition.slot_id)
                    .expect("invariant: admitted resume target contains every addition");
                Ok(PendingMemberDirectory {
                    disk_id: disk.disk_id.clone(),
                    directory: crate::storage_directories::reopen_fresh_addition(
                        &addition.selection,
                        &stopped,
                    )?,
                })
            })
            .collect()
    }

    fn acquire_terminal_resume(
        self: &Arc<Self>,
        identity: &Identity,
        intent: &mut StoredAddition,
        directories: Option<Vec<PendingMemberDirectory>>,
        directory: &PrivateDirectory,
    ) -> Result<ResumeOwner, ApiError> {
        self.current_management_identity(identity)?;
        // An Engine can already own the permanent result after auth replication
        // or the HTTP record write failed. Never initialize or prepare again.
        if crate::lock(&self.state).backend.engine().is_some() {
            self.recover_once()?;
            return self
                .terminal_resume_result(identity, intent)?
                .ok_or_else(|| {
                    recovery("the recovered Engine has no matching metadata-resume publication")
                })
                .map(|result| ResumeOwner::Published(Box::new(result)));
        }
        let previous = {
            let mut state = crate::lock(&self.state);
            if let BackendState::MetadataResume(pending) = &state.backend {
                require_resume_owner(pending, intent)?;
            }
            state.admitted = false;
            state.status = StorageState::RecoveryRequired;
            std::mem::replace(&mut state.backend, BackendState::ChangingAuthority)
        };
        let pending = match previous {
            BackendState::Terminal(owner) => {
                let terminal = match Arc::try_unwrap(owner) {
                    Ok(terminal) => terminal,
                    Err(owner) => {
                        crate::lock(&self.state).backend = BackendState::Terminal(owner);
                        return Err(ApiError::conflict(
                            ErrorCode::StorageDraining,
                            "existing management reads must finish; continue the original metadata resume",
                        ));
                    }
                };
                self.prepare_terminal_resume(identity, intent, terminal, directories, directory)
                    .map(|pending| ResumeOwner::Pending(Box::new(pending)))
            }
            BackendState::MetadataResume(pending) => Ok(ResumeOwner::Pending(pending)),
            BackendState::ResumeRecovery | BackendState::Unopened => {
                self.reopen_terminal_resume(identity, intent, directories, directory)
            }
            other => {
                crate::lock(&self.state).backend = other;
                return Err(recovery(
                    "another authority handover owns the storage backend",
                ));
            }
        };
        match pending {
            Ok(owner) => Ok(owner),
            Err(error) => {
                if matches!(
                    crate::lock(&self.state).backend,
                    BackendState::ChangingAuthority
                ) {
                    crate::lock(&self.state).backend = BackendState::ResumeRecovery;
                }
                Err(error)
            }
        }
    }

    fn run_terminal_resume(
        self: &Arc<Self>,
        identity: &Identity,
        intent: &mut StoredAddition,
        directories: Option<Vec<PendingMemberDirectory>>,
        directory: &PrivateDirectory,
    ) -> Result<OperationResult, ApiError> {
        let mut pending =
            match self.acquire_terminal_resume(identity, intent, directories, directory)? {
                ResumeOwner::Pending(pending) => *pending,
                ResumeOwner::Published(result) => return Ok(*result),
            };
        // The pending owner is put back on EVERY pre-activation error. Its
        // actual locks/FDs are not dropped because an HTTP save or auth failed.
        let prepared = (|| {
            self.current_management_identity(identity)?;
            bind_held_resume(&pending, intent)?;
            intent.status.phase = AdditionPhase::JointDecision;
            save(directory, intent)?;
            self.with_resume_authorization(identity, || pending.synchronize())?;
            self.current_management_identity(identity)?;
            intent.status.phase = AdditionPhase::Activating;
            save(directory, intent)?;
            self.current_management_identity(identity)?;
            Ok(())
        })();
        if let Err(error) = prepared {
            self.restore_resume(pending);
            return Err(error);
        }
        let auth = crate::lock(&self.auth)
            .clone()
            .expect("invariant: prepared resume has management auth");
        let current = crate::lock(&auth);
        if let Err(error) = self
            .ensure_running()
            .and_then(|()| current.revalidate_identity(identity).map(|_| ()))
        {
            drop(current);
            self.restore_resume(pending);
            return Err(error);
        }
        let activated = pending
            .activate_with_workspace_limits(self.retained_workspace_limits)
            .map_err(crate::engine_backend::error);
        drop(current);
        let engine = match activated {
            Ok(engine) => engine,
            Err(error) => {
                crate::lock(&self.state).backend = BackendState::ResumeRecovery;
                return Err(error);
            }
        };
        self.install_resumed_engine(identity, intent, engine)
    }

    fn prepare_terminal_resume(
        &self,
        identity: &Identity,
        intent: &mut StoredAddition,
        terminal: TerminalManagement,
        directories: Option<Vec<PendingMemberDirectory>>,
        directory: &PrivateDirectory,
    ) -> Result<TerminalMetadataResume, ApiError> {
        let status = terminal.status();
        let expected = AdditionAuthority::TerminalManagement {
            terminal_authority_hash: status.authority_hash,
            terminal_operation_id: status.operation_id,
        };
        if intent.status.preview.authority != expected
            || configuration_digest(terminal.original_configuration())
                != intent.status.preview.configuration_digest
        {
            // Return the still usable owner after rejecting a stale preview.
            crate::lock(&self.state).backend = BackendState::Terminal(Arc::new(terminal));
            return Err(ApiError::conflict(
                ErrorCode::GenerationMismatch,
                "terminal authority changed since this preview",
            ));
        }
        let archive = terminal.control_archive();
        // Before consuming terminal, errors retain the actual live authority.
        let ready = (|| {
            self.current_management_identity(identity)?;
            let held = match directories {
                Some(held) => held,
                None => self.open_resume_directories(intent)?,
            };
            intent.status.phase = AdditionPhase::Enrolling;
            save(directory, intent)?;
            self.current_management_identity(identity)?;
            Ok(held)
        })();
        let directories = match ready {
            Ok(held) => held,
            Err(error) => {
                crate::lock(&self.state).backend = BackendState::Terminal(Arc::new(terminal));
                return Err(error);
            }
        };
        let pending = self.with_resume_authorization(identity, || {
            terminal.prepare_metadata_resume(
                intent.target.clone(),
                &identity.principal,
                intent.status.operation_id,
                directories,
            )
        })?;
        let bound = (|| {
            let status = pending.status().map_err(crate::engine_backend::error)?;
            let binding = ArchiveBinding {
                archive_path: archive.archive_path,
                root_hash: archive.root_hash,
                transition_hash: status.transition_hash,
            };
            if intent
                .archive
                .as_ref()
                .is_some_and(|saved| saved != &binding)
            {
                return Err(recovery(
                    "prepared metadata resume differs from its saved archive",
                ));
            }
            intent.archive = Some(binding);
            save(directory, intent)
        })();
        if let Err(error) = bound {
            self.restore_resume(pending);
            return Err(error);
        }
        Ok(pending)
    }

    fn reopen_terminal_resume(
        self: &Arc<Self>,
        identity: &Identity,
        intent: &mut StoredAddition,
        directories: Option<Vec<PendingMemberDirectory>>,
        directory: &PrivateDirectory,
    ) -> Result<ResumeOwner, ApiError> {
        let proof = TerminalManagement::metadata_resume_recovery(&self.data_root)
            .map_err(crate::engine_backend::error)?;
        let Some(proof) = proof else {
            let terminal =
                TerminalManagement::open(&self.data_root).map_err(crate::engine_backend::error)?;
            if let Some(result) = terminal.operation(intent.status.operation_id) {
                if terminal
                    .state()
                    .operation_principals
                    .get(&intent.status.operation_id)
                    .map(String::as_str)
                    != Some(identity.principal.as_str())
                {
                    self.install_terminal(terminal);
                    return Err(ApiError::forbidden());
                }
                let result = exact_result(result.clone(), intent);
                self.install_terminal(terminal);
                return result.map(|result| ResumeOwner::Published(Box::new(result)));
            }
            return self
                .prepare_terminal_resume(identity, intent, terminal, directories, directory)
                .map(|pending| ResumeOwner::Pending(Box::new(pending)));
        };
        self.bind_resume_recovery(&proof, intent)?;
        save(directory, intent)?;
        self.current_management_identity(identity)?;
        match proof.phase {
            TerminalMetadataResumePhase::ResupplyDirectories => {
                let management = self.with_resume_authorization(identity, || {
                    TerminalManagement::open_metadata_resume_checked(&self.data_root, &proof)
                })?;
                let held = match directories {
                    Some(held) => held,
                    None => self.open_resume_directories(intent)?,
                };
                self.with_resume_authorization(identity, || {
                    management.reopen_metadata_resume_held(held)
                })
                .map(|pending| ResumeOwner::Pending(Box::new(pending)))
            }
            TerminalMetadataResumePhase::CompleteGeneration => {
                if intent.status.continuation == AdditionContinuation::Cancel {
                    // Cancellation cannot implicitly publish a selected resume.
                    // Keep the protected selector for an explicit Add retry.
                    intent.status.phase = AdditionPhase::ResumeQuorumAccepted;
                    return Err(recovery(
                        "the selected metadata resume cannot be cancelled; explicitly continue the original recovery",
                    ));
                }
                // Activation consumed its pending owner. Restore through the
                // protected selector, not the HTTP progress field.
                let engine = self.with_resume_authorization(identity, || {
                    TerminalManagement::open_metadata_resume_checked(&self.data_root, &proof)?
                        .complete_metadata_resume_held(self.retained_workspace_limits)
                })?;
                self.install_resumed_engine(identity, intent, engine)
                    .map(|result| ResumeOwner::Published(Box::new(result)))
            }
        }
    }

    fn install_resumed_engine(
        self: &Arc<Self>,
        identity: &Identity,
        intent: &mut StoredAddition,
        engine: storage_engine::Engine,
    ) -> Result<OperationResult, ApiError> {
        let backend = Arc::new(EngineBackend::new(engine));
        {
            let mut state = crate::lock(&self.state);
            state.backend = BackendState::Engine(backend);
            state.admitted = false;
            state.status = StorageState::RecoveryRequired;
        }
        // Normal current-system auth replication and admission is shared with
        // startup. The archived terminal credentials never replace primary auth.
        self.recover_once()?;
        self.terminal_resume_result(identity, intent)?
            .ok_or_else(|| recovery("resumed Engine did not admit its exact permanent operation"))
    }
}

impl ManagementBackend {
    pub(super) fn execute_terminal_resume(
        &self,
        identity: &Identity,
        directory: &PrivateDirectory,
        mut record: StoredAddition,
        held: Option<Vec<PendingMemberDirectory>>,
    ) -> Result<Value, ApiError> {
        self.inner.current_management_identity(identity)?;
        if let Some(result) = self.inner.terminal_resume_result(identity, &record)? {
            finish(&mut record, result);
            save(directory, &record)?;
            return Ok(
                json_codec::to_value(record.status).expect("invariant: addition status serializes")
            );
        }
        if let Some(latest) = read::<OperationId>(directory, &latest_name())?
            && latest != record.status.operation_id
        {
            self.require_settled_addition(directory)?;
        }
        if self.inner.recovering.swap(true, Ordering::SeqCst) {
            return Err(ApiError::conflict(
                ErrorCode::StorageRetryRunning,
                "storage recovery is already running",
            ));
        }
        let _recovery = RecoveryPermit(&self.inner.recovering);
        record.status.result = None;
        record.status.state = match record.status.continuation {
            AdditionContinuation::Add => AdditionState::Adding,
            AdditionContinuation::Cancel => AdditionState::Cancelling,
        };
        record.status.failure = None;
        if matches!(
            record.status.phase,
            AdditionPhase::Completed | AdditionPhase::Cancelled
        ) {
            record.status.phase = match record.status.continuation {
                AdditionContinuation::Add => AdditionPhase::Activating,
                AdditionContinuation::Cancel => AdditionPhase::CertifyingAbort,
            };
        }
        save(directory, &record)?;
        write(directory, &latest_name(), &record.status.operation_id)?;
        let outcome = match record.status.continuation {
            AdditionContinuation::Add => {
                self.inner
                    .run_terminal_resume(identity, &mut record, held, directory)
            }
            AdditionContinuation::Cancel => {
                self.inner
                    .run_terminal_resume_cancel(identity, &mut record, directory)
            }
        };
        let result = match outcome {
            Ok(result) => result,
            Err(error) => {
                // Auth or HTTP persistence can fail after core publication.
                // Only its actual receipt changes the uncertain outcome.
                if let Some(result) = self.inner.terminal_resume_result(identity, &record)? {
                    result
                } else {
                    record.status.state = AdditionState::RecoveryRequired;
                    // An observation failure must not replace the original
                    // error or imply that cancellation is available.
                    record.status.phase = self
                        .inner
                        .terminal_resume_cancellation_phase(&record)
                        .unwrap_or(AdditionPhase::ResumeCancellationUnknown);
                    record.status.failure = Some(AdditionFailure {
                        code: error.code,
                        message: error.message.clone(),
                    });
                    save(directory, &record)?;
                    return Err(ApiError::new(
                        http::StatusCode::SERVICE_UNAVAILABLE,
                        ErrorCode::RecoveryRequired,
                        format!(
                            "metadata resume {} needs recovery; continue the original operation",
                            record.status.operation_id
                        ),
                    )
                    .with_source(Arc::new(error)));
                }
            }
        };
        finish(&mut record, result);
        save(directory, &record)?;
        Ok(json_codec::to_value(record.status).expect("invariant: addition status serializes"))
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/management/terminal_resume.rs"]
mod tests;
