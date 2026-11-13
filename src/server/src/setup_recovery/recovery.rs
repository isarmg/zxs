//! Explicit SYSTEM-loss recovery binds source authority to one new held carrier.
use super::{SetupRecovery, RestoreRequest, engine_error, control_publication_error};
use super::storage_system::StorageSystemBinding;
use crate::ApiError;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::PrivateDirectory;
use storage_contract::{Principal, OperationId, working_tree::MaterialCommitment};
use storage_engine::Engine;

pub(super) const RECOVERY_INTENT: &str = "storage-recovery-intent.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
enum RecoveryFormat { BoundSystemRecoveryV1 }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct StorageRecoveryIntent {
    format: RecoveryFormat,
    pub(super) storage_system: StorageSystemBinding,
    source_evidence: MaterialCommitment,
    principal: Principal,
    disk_ids: Vec<String>,
}

impl StorageRecoveryIntent {
    fn commitment(&self) -> Result<MaterialCommitment, ApiError> {
        let bytes = storage_contract::canonical_json(self).map_err(ApiError::invalid_source)?;
        let mut hash = blake3::Hasher::new_derive_key("zxs storage SYSTEM recovery intent v1");
        hash.update(&bytes);
        Ok(MaterialCommitment::from_digest(hash.finalize()))
    }
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
        // Existing initialization owners need an explicit origin-linked successor
        // protocol. This first recovery entry never replaces or adopts that owner.
        for name in ["setup-intent.json", super::activation::ACTIVATION_INTENT, "config.json"] {
            if super::activation::present(&self.data_root.join(name))? { return Err(refused()); }
        }
        let existing = if super::activation::present(&self.data_root.join(RECOVERY_INTENT))? {
            let intent: StorageRecoveryIntent = self.read_durable_control(RECOVERY_INTENT)?;
            if intent.principal != principal || intent.disk_ids != request.disk_ids {
                return Err(refused());
            }
            Some(intent)
        } else {
            // An unbound child, even empty, is not a reusable recovery target.
            if self.storage_system_if_bound()?.is_some() { return Err(refused()); }
            None
        };
        let paths = self.selected(&request.disk_ids)?;
        let directories = self.selected_held(&request.disk_ids)?.into_iter().zip(paths)
            .map(|(file, path)| PrivateDirectory::from_directory(file, path)
                .map_err(control_publication_error)).collect::<Result<Vec<_>, _>>()?;
        let admission = Engine::admit_storage_recovery(directories).map_err(engine_error)?;
        // Source admission can outlive authorization. No destination is created
        // and no durable control recovery intent is published before this check.
        revalidate()?;
        let (intent, system) = match existing {
            Some(intent) => {
                let system = intent.storage_system.open(&self.data_root)?;
                (intent, system)
            }
            None => {
                let operation = OperationId::generate().map_err(ApiError::invalid_source)?;
                let (storage_system, system) = StorageSystemBinding::create(
                    &self.control_directory, operation,
                )?;
                let intent = StorageRecoveryIntent {
                    format: RecoveryFormat::BoundSystemRecoveryV1, storage_system,
                    source_evidence: admission.evidence_commitment(), principal: principal.clone(),
                    disk_ids: request.disk_ids,
                };
                self.write_control(RECOVERY_INTENT, &intent)?;
                (intent, system)
            }
        };
        let commitment = intent.commitment()?;
        admission.require_recovery_intent_source(intent.storage_system.operation,
            &principal, intent.source_evidence, commitment).map_err(engine_error)?;
        revalidate()?;
        let mut engine = Engine::restore_storage_held(system, intent.storage_system.operation,
            principal, admission, commitment, self.retained_workspace_limits).map_err(engine_error)?;
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
