//! Explicit SYSTEM-loss recovery binds source authority to one new held carrier.
use super::{SetupRecovery, RestoreRequest, engine_error, control_publication_error};
use super::storage_system::StorageSystemBinding;
use crate::ApiError;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::PrivateDirectory;
use storage_contract::{Principal, OperationId, working_tree::MaterialCommitment};
use storage_engine::Engine;

pub(super) const RECOVERY_INTENT: &str = "storage-recovery-intent.json";

mod predecessor;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct StorageRecoveryIntent {
    pub(super) control: storage_engine::StorageRecoveryControl,
}

fn refused() -> ApiError {
    ApiError::conflict(storage_contract::ErrorCode::SetupProofInvalid,
        "SYSTEM recovery requires its exact original control intent and source authority")
}

impl SetupRecovery {
    pub(super) fn restore_locked<F, V>(
        &self, request: RestoreRequest, principal: Principal, revalidate: V, restore: F,
    ) -> Result<(), ApiError>
    where F: FnOnce(Engine) -> Result<(), ApiError>, V: Fn() -> Result<(), ApiError>,
    {
        if request.disk_ids.is_empty() || request.disk_ids.len() > 32 {
            return Err(ApiError::invalid("restore requires authorized metadata members"));
        }
        self.control_directory.verify_identity().map_err(control_publication_error)?;
        let predecessor = self.capture_recovery_predecessor()?;
        let existing = if super::activation::present(&self.data_root.join(RECOVERY_INTENT))? {
            let intent: StorageRecoveryIntent = self.read_durable_control(RECOVERY_INTENT)?;
            self.require_recovery_predecessor(&intent.control.predecessor)?;
            if intent.control.principal != principal || intent.control.disk_ids != request.disk_ids {
                return Err(refused());
            }
            Some(intent)
        } else {
            match &predecessor {
                storage_engine::StorageRecoveryPredecessor::NewControl => {
                    if self.storage_system_if_bound()?.is_some() { return Err(refused()); }
                }
                storage_engine::StorageRecoveryPredecessor::Initialization { evidence } => {
                    StorageSystemBinding::from_evidence(&evidence.binding)
                        .require_missing_original(&self.data_root)?;
                }
            }
            None
        };
        let paths = self.selected(&request.disk_ids)?;
        let directories = self.selected_held(&request.disk_ids)?.into_iter().zip(paths)
            .map(|(file, path)| PrivateDirectory::from_directory(file, path)
                .map_err(control_publication_error)).collect::<Result<Vec<_>, _>>()?;
        let admission = Engine::admit_storage_recovery(directories).map_err(engine_error)?;
        if let storage_engine::StorageRecoveryPredecessor::Initialization { evidence } = &predecessor {
            admission.require_initialization_binding(evidence.binding.operation,
                &evidence.configuration, evidence.binding.system).map_err(engine_error)?;
        }
        // Source admission can outlive authorization. No destination is created
        // and no durable control recovery intent is published before this check.
        revalidate()?;
        let (intent, system) = match existing {
            Some(intent) => {
                let system = StorageSystemBinding::from_evidence(&intent.control.binding).open(&self.data_root)?;
                (intent, system)
            }
            None => {
                let operation = OperationId::generate().map_err(ApiError::invalid_source)?;
                let (storage_system, system) = StorageSystemBinding::create(
                    &self.control_directory, operation,
                )?;
                self.require_recovery_predecessor(&predecessor)?;
                let intent = StorageRecoveryIntent {
                    control: storage_engine::StorageRecoveryControl::new(
                        storage_system.evidence(), admission.evidence_commitment(), principal.clone(),
                        request.disk_ids, predecessor,
                    ),
                };
                self.write_control(RECOVERY_INTENT, &intent)?;
                (intent, system)
            }
        };
        let commitment = intent.control.commitment().map_err(engine_error)?;
        admission.require_recovery_intent_source(intent.control.binding.operation,
            &principal, intent.control.source_evidence, commitment).map_err(engine_error)?;
        revalidate()?;
        let mut engine = Engine::restore_storage_held(system, intent.control.binding.operation,
            principal, admission, commitment, intent.control.clone(), self.retained_workspace_limits).map_err(engine_error)?;
        self.require_recovered_engine(&engine)?;
        crate::engine_backend::report_recovery_warnings(&mut engine);
        if engine.auth_state().is_none() {
            return Err(ApiError::conflict(storage_contract::ErrorCode::AuthRecoveryRequired,
                "restored storage has no authoritative credentials; explicit recovery remains required"));
        }
        #[cfg(test)]
        super::tests::after_restore();
        revalidate()?;
        restore(engine)
    }
}
