//! No backup stage machine lives here. Core methods retain their native proofs.
use super::{AdmissionEngine, EngineBackend, error, operations};
use crate::{ApiError, auth::Identity};
use std::path::{Path, PathBuf};
use storage_contract::{CommitState, ErrorCode, OperationId, backup_transaction as wire};
use storage_engine::{
    BackupPreparationPhase, BackupPreparationRequest, BackupTopologyTarget,
    BackupTransactionCancelOutcome, BackupTransactionCancellation, BackupTransactionRequest,
};

fn administrator(engine: &AdmissionEngine, caller: &Identity) -> Result<Identity, ApiError> {
    let current = engine.managed_identity(caller)?;
    if current.admission_session.is_none() || current.smb_binding.is_some() {
        return Err(ApiError::forbidden());
    }
    Ok(current)
}

impl EngineBackend {
    pub(crate) fn backup_transaction_context(
        &self,
        identity: &Identity,
    ) -> Result<(u64, Vec<String>), ApiError> {
        let engine = crate::lock(&self.engine);
        administrator(&engine, identity)?;
        let enabled: std::collections::BTreeSet<_> = engine
            .registered_disks()
            .filter(|disk| disk.registration_mode == storage_engine::RegistrationMode::Enabled)
            .map(|disk| disk.disk_id)
            .collect();
        let slots = engine
            .config()
            .disks
            .iter()
            // An intermediate source can replace its terminal downstream edge.
            // This is an inventory, not a duplicate of the core's exact full-
            // chain/terminal-edge eligibility checks.
            .filter(|disk| enabled.contains(disk.disk_id.as_str()))
            .map(|disk| disk.slot_id.clone())
            .collect();
        Ok((engine.state().sequence, slots))
    }

    pub(crate) fn current_backup_transaction(
        &self,
        identity: &Identity,
    ) -> Result<wire::Current, ApiError> {
        let engine = crate::lock(&self.engine);
        let current = administrator(&engine, identity)?;
        let request = engine
            .current_backup_transaction_request(&current.principal)
            .map_err(error)?
            .map(original_wire)
            .transpose()?;
        let transaction = match (&request, engine.backup_preparation_status()) {
            (Some(request), _) => Some(snapshot(&engine, &current, request.operation_id)?),
            (None, None) => None,
            (None, Some(_)) => {
                return Err(ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "a retained legacy preparation cannot be replaced by a new user transaction",
                ));
            }
        };
        Ok(wire::Current {
            request,
            transaction,
        })
    }

    pub(crate) fn backup_transaction_status(
        &self,
        identity: &Identity,
        operation: OperationId,
    ) -> Result<wire::Snapshot, ApiError> {
        let engine = crate::lock(&self.engine);
        let current = administrator(&engine, identity)?;
        snapshot(&engine, &current, operation)
    }

    pub(crate) fn start_backup_transaction(
        &self,
        identity: &Identity,
        request: wire::StartRequest,
        discovery: &crate::storage_directories::DiscoveryRoot,
        management_root: &Path,
    ) -> Result<wire::Snapshot, ApiError> {
        if !(1..=8).contains(&request.targets.len()) {
            return Err(ApiError::invalid(
                "choose one to eight independent complete-copy targets",
            ));
        }
        let mut engine = crate::lock(&self.engine);
        let current = administrator(&engine, identity)?;
        let original = core_request(&request, &current.principal);
        let operation = request.operation_id;
        if let Some(retained) = engine
            .backup_transaction_request(operation, &current.principal)
            .map_err(error)?
        {
            if retained != original {
                return Err(ApiError::conflict(
                    ErrorCode::IdempotencyKeyReused,
                    "the original backup input cannot be changed or rebound",
                ));
            }
            // The original directories are no longer fresh. An exact retry is
            // read-only; explicit Resume uses the core's retained physical binding.
            return snapshot(&engine, &current, operation);
        }
        let directories = open_targets(&engine, &request, discovery, management_root)?;
        // Opening may block on host I/O. Revalidate the actual session immediately
        // before the first durable operation while the same Engine baseline is held.
        administrator(&engine, identity)?;
        engine
            .start_backup_transaction(original, directories)
            .map_err(error)?;
        snapshot(&engine, &current, operation)
    }

    pub(crate) fn continue_backup_transaction(
        &self,
        identity: &Identity,
        operation: OperationId,
        command: wire::Command,
    ) -> Result<wire::CommandResponse, ApiError> {
        let mut engine = crate::lock(&self.engine);
        let current = administrator(&engine, identity)?;
        // Cancellation can also tombstone an exact Start which never reached
        // SYSTEM. Every other action still requires the retained original task.
        if !matches!(&command, wire::Command::CancelPreparation { .. }) {
            require_request(&engine, operation, &current.principal)?;
        }
        match command {
            wire::Command::CancelPreparation {
                confirmation: wire::CancellationConfirmation::PreserveFilesAndCancelBeforeSourceStop,
                original_start,
            } => {
                let original = original_start
                    .as_ref()
                    .map(|request| core_request(request, &current.principal));
                let outcome = engine
                    .cancel_backup_transaction(operation, &current.principal, original)
                    .map_err(error)?;
                let snapshot = snapshot(&engine, &current, operation)?;
                return Ok(match outcome {
                    BackupTransactionCancelOutcome::Cancelled => {
                        wire::CommandResponse::Observed { snapshot }
                    }
                    BackupTransactionCancelOutcome::ContinuationRequired => {
                        wire::CommandResponse::CancellationRejected { snapshot }
                    }
                });
            }
            wire::Command::ContinuePreparation {} => {
                engine
                    .continue_backup_transaction(operation, &current.principal)
                    .map_err(error)?;
            }
            wire::Command::ResumeOriginalTask {
                confirmation: wire::ResumeConfirmation::RecoverOnlyTheOriginalRetainedTask,
            } => {
                engine
                    .resume_backup_transaction(operation, &current.principal)
                    .map_err(error)?;
            }
            wire::Command::FenceSource {
                confirmation: wire::StopConfirmation::StopAllOriginalPoolWrites,
            } => {
                engine
                    .begin_backup_cutover_fence(operation, &current.principal)
                    .map_err(error)?;
            }
            wire::Command::DecideRegistry {
                confirmation: wire::DecisionConfirmation::SelectOriginalIrreversibleRegistryDecision,
            } => {
                engine
                    .start_backup_registry_placement_decision(operation, &current.principal)
                    .map_err(error)?;
            }
            wire::Command::Activate {
                confirmation: wire::ActivationConfirmation::ActivateOnlyTheOriginalPublishedBackup,
            } => {
                engine
                    .complete_backup_registry_placement_decision(operation, &current.principal)
                    .map_err(error)?;
            }
        }
        Ok(wire::CommandResponse::Observed {
            snapshot: snapshot(&engine, &current, operation)?,
        })
    }
}

fn core_request(request: &wire::StartRequest, principal: &str) -> BackupTransactionRequest {
    BackupTransactionRequest {
        preparation: BackupPreparationRequest {
            operation_id: request.operation_id,
            principal: principal.into(),
            source_slot_id: request.source_slot_id.clone(),
            target_paths: request
                .targets
                .iter()
                .map(|target| PathBuf::from(&target.selection.root_path))
                .collect(),
            expected_sequence: request.expected_sequence,
        },
        targets: request
            .targets
            .iter()
            .map(|target| BackupTopologyTarget {
                disk_id: target.disk_id.clone(),
                slot_id: target.slot_id.clone(),
                upstream_slot_id: target.upstream_slot_id.clone(),
            })
            .collect(),
    }
}

fn open_targets(
    engine: &AdmissionEngine,
    request: &wire::StartRequest,
    discovery: &crate::storage_directories::DiscoveryRoot,
    management_root: &Path,
) -> Result<Vec<std::fs::File>, ApiError> {
    let mut excluded = vec![engine.root().to_path_buf(), management_root.to_path_buf()];
    excluded.extend(engine.config().disks.iter().map(|disk| disk.root.clone()));
    let enabled: std::collections::BTreeSet<_> = engine
        .registered_disks()
        .filter(|disk| disk.registration_mode == storage_engine::RegistrationMode::Enabled)
        .map(|disk| disk.disk_id)
        .collect();
    let stopped: Vec<_> = engine
        .config()
        .disks
        .iter()
        .filter(|disk| !enabled.contains(disk.disk_id.as_str()))
        .map(|disk| disk.root.clone())
        .collect();
    let mut held = Vec::with_capacity(request.targets.len());
    for target in &request.targets {
        let directory = crate::storage_directories::revalidate_fresh_within(
            discovery.as_str(),
            &target.selection,
            &excluded,
            &stopped,
        )?;
        excluded.push(PathBuf::from(&target.selection.root_path));
        held.push(directory);
    }
    Ok(held
        .into_iter()
        .map(|directory| directory.into_storage_directory())
        .collect())
}

fn require_request(
    engine: &AdmissionEngine,
    operation: OperationId,
    principal: &str,
) -> Result<BackupTransactionRequest, ApiError> {
    engine
        .backup_transaction_request(operation, principal)
        .map_err(error)?
        .ok_or_else(|| {
            ApiError::new(
                http::StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                "the original backup transaction was not found",
            )
        })
}

fn snapshot(
    engine: &AdmissionEngine,
    current: &Identity,
    operation: OperationId,
) -> Result<wire::Snapshot, ApiError> {
    let original = require_request(engine, operation, &current.principal)?;
    if let Some(cancellation) = engine
        .backup_transaction_cancellation(operation, &current.principal)
        .map_err(error)?
    {
        let (observation, stage, detail) = match cancellation {
            BackupTransactionCancellation::Pending => (
                wire::Observation::CancellationPending,
                "CancellationPending",
                "cancellation is retained; explicitly retry cancellation to stop the original worker and finish preserving its task",
            ),
            BackupTransactionCancellation::Cancelled => (
                wire::Observation::Cancelled,
                "Cancelled",
                "preparation was cancelled and archived; all original and target bytes were preserved; a new task requires a new operation and empty targets",
            ),
        };
        return inactive_snapshot(original, observation, stage, None, detail);
    }
    let Some(status) = engine
        .backup_preparation_status()
        .filter(|status| status.operation_id == operation)
    else {
        return inactive_snapshot(
            original,
            wire::Observation::Blocked,
            "RetainedRequest",
            Some(ErrorCode::RecoveryRequired),
            "the original input is retained but its runtime observation is unavailable; resume this same operation or explicitly cancel before source stop",
        );
    };
    let observation = match status.phase {
        BackupPreparationPhase::Blocked => wire::Observation::Blocked,
        BackupPreparationPhase::SourceChanged => wire::Observation::SourceChanged,
        BackupPreparationPhase::GenerationServingActive => wire::Observation::ServingActive,
        BackupPreparationPhase::Copying
        | BackupPreparationPhase::Verifying
        | BackupPreparationPhase::HistoryPrepared
        | BackupPreparationPhase::NativeControlPrepared
        | BackupPreparationPhase::ObjectDatabasesPrepared
        | BackupPreparationPhase::ScopedDatabasesPrepared
        | BackupPreparationPhase::CutoverInputsPrepared
        | BackupPreparationPhase::TargetEntitiesPrepared
        | BackupPreparationPhase::RegistryPlacementDecided
        | BackupPreparationPhase::WriterRoutesPrepared
        | BackupPreparationPhase::GenerationInputsPrepared
        | BackupPreparationPhase::GenerationControlPrepared
        | BackupPreparationPhase::GenerationAuthorityPrepared
        | BackupPreparationPhase::GenerationFirstCandidatePrepared
        | BackupPreparationPhase::GenerationFirstCandidateFinalized
        | BackupPreparationPhase::GenerationFirstCandidatePublished
        | BackupPreparationPhase::GenerationFirstPublicationRetained
        | BackupPreparationPhase::GenerationConfigurationResolved
        | BackupPreparationPhase::GenerationRegistryInstalled
        | BackupPreparationPhase::GenerationServingImagesInstalled
        | BackupPreparationPhase::GenerationPublishedScopesInstalled
        | BackupPreparationPhase::GenerationCommonDatabasesInstalled
        | BackupPreparationPhase::GenerationCurrentReadAdmitted
        | BackupPreparationPhase::GenerationDatabaseAliasesInstalled
        | BackupPreparationPhase::GenerationOrdinaryRootDatabasesInstalled
        | BackupPreparationPhase::GenerationOrdinarySystemDatabaseInstalled
        | BackupPreparationPhase::GenerationOrdinaryNativeMetadataInstalled
        | BackupPreparationPhase::GenerationTargetWriterFencesInstalled
        | BackupPreparationPhase::GenerationTargetWriterIoPrepared
        | BackupPreparationPhase::GenerationTargetSealsInstalled
        | BackupPreparationPhase::GenerationTargetLocatorsInstalled
        | BackupPreparationPhase::GenerationOriginalControlInstalled
        | BackupPreparationPhase::GenerationOrdinaryServingInstalled => {
            wire::Observation::Preparing
        }
    };
    let completion = if observation == wire::Observation::ServingActive {
        let record = operations::authorized_record(engine, current, operation)?;
        let result = record.result();
        if result.operation_id != operation
            || result.kind != "backup-target-replacement"
            || !matches!(
                result.commit_state,
                CommitState::Published | CommitState::Acked
            )
        {
            return Err(ApiError::conflict(
                ErrorCode::StateInvalid,
                "active backup does not have its original published completion receipt",
            ));
        }
        Some(result.clone())
    } else {
        None
    };
    Ok(wire::Snapshot {
        request: original_wire(original)?,
        progress: wire::Progress {
            observation,
            stage: format!("{:?}", status.phase),
            completed_files: status.completed_files,
            total_files: status.total_files,
            completed_bytes: status.completed_bytes,
            total_bytes: status.total_bytes,
            error_code: status.error_code,
            detail: status.detail,
        },
        completion,
    })
}

fn inactive_snapshot(
    original: BackupTransactionRequest,
    observation: wire::Observation,
    stage: &str,
    error_code: Option<ErrorCode>,
    detail: &str,
) -> Result<wire::Snapshot, ApiError> {
    Ok(wire::Snapshot {
        request: original_wire(original)?,
        progress: wire::Progress {
            observation,
            stage: stage.into(),
            completed_files: 0,
            total_files: 0,
            completed_bytes: 0,
            total_bytes: 0,
            error_code,
            detail: detail.into(),
        },
        completion: None,
    })
}

fn original_wire(original: BackupTransactionRequest) -> Result<wire::OriginalRequest, ApiError> {
    let BackupTransactionRequest {
        preparation,
        targets,
    } = original;
    if preparation.target_paths.len() != targets.len() {
        return Err(ApiError::conflict(
            ErrorCode::StateInvalid,
            "the retained target order is incomplete",
        ));
    }
    let targets = preparation
        .target_paths
        .into_iter()
        .zip(targets)
        .map(|(path, target)| {
            let root_path = path
                .to_str()
                .ok_or_else(|| ApiError::invalid("backup target is not UTF-8"))?
                .to_owned();
            Ok(wire::OriginalTarget {
                root_path,
                disk_id: target.disk_id,
                slot_id: target.slot_id,
                upstream_slot_id: target.upstream_slot_id,
            })
        })
        .collect::<Result<_, ApiError>>()?;
    Ok(wire::OriginalRequest {
        operation_id: preparation.operation_id,
        source_slot_id: preparation.source_slot_id,
        expected_sequence: preparation.expected_sequence,
        targets,
    })
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/backup_transaction.rs"]
mod tests;
