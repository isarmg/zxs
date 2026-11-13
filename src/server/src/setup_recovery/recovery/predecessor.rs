//! Preserve the exact original control owner instead of replacing its authority.
use super::*;
use crate::setup_recovery::{SetupIntent, SetupReady, activation};
use storage_engine::{StorageControlOwner, StorageOwnerEvidence, StorageRecoveryPredecessor};

fn evidence(bytes: &[u8]) -> MaterialCommitment {
    let mut hash = blake3::Hasher::new_derive_key("zxs retained control owner bytes v1");
    hash.update(bytes);
    MaterialCommitment::from_digest(hash.finalize())
}

impl SetupRecovery {
    pub(in crate::setup_recovery) fn capture_recovery_predecessor(
        &self,
    ) -> Result<StorageRecoveryPredecessor, ApiError> {
        use activation::present;
        if present(&self.data_root.join("config.json"))? { return Err(refused()); }
        let setup = present(&self.data_root.join("setup-intent.json"))?;
        let activate = present(&self.data_root.join(activation::ACTIVATION_INTENT))?;
        let (owner, binding, configuration, intent, ready, complete) = match (setup, activate) {
            (false, false) => {
                for name in ["setup-ready.json", "storage-activation-ready.json", "storage-activation-complete.json"] {
                    if present(&self.data_root.join(name))? { return Err(refused()); }
                }
                return Ok(StorageRecoveryPredecessor::NewControl);
            },
            (true, true) => return Err(refused()),
            (true, false) => {
                let intent = self.read_durable_control_bytes("setup-intent.json")?;
                let original: SetupIntent = storage_contract::decode_strict(&intent)
                    .map_err(ApiError::invalid_source)?;
                let ready = self.read_durable_control_bytes("setup-ready.json")?;
                let proof: SetupReady = storage_contract::decode_strict(&ready)
                    .map_err(ApiError::invalid_source)?;
                if original.version != 2
                    || proof.intent_digest != crate::setup_recovery::value_digest(&original)? {
                    return Err(refused());
                }
                let complete = self.read_durable_control_bytes("management-authority.json")?;
                if complete != crate::management::MANAGEMENT_AUTHORITY_BYTES {
                    return Err(refused());
                }
                (StorageControlOwner::Setup, original.storage_system.evidence(),
                    (*original.config).clone(), intent, ready, complete)
            }
            (false, true) => {
                let intent = self.read_durable_control_bytes(activation::ACTIVATION_INTENT)?;
                let original: activation::StorageActivationIntent = storage_contract::decode_strict(&intent)
                    .map_err(ApiError::invalid_source)?;
                let ready = self.read_durable_control_bytes("storage-activation-ready.json")?;
                let ready_proof: activation::StorageActivationReady = storage_contract::decode_strict(&ready)
                    .map_err(ApiError::invalid_source)?;
                let complete = self.read_durable_control_bytes("storage-activation-complete.json")?;
                let proof: activation::StorageActivationComplete = storage_contract::decode_strict(&complete)
                    .map_err(ApiError::invalid_source)?;
                let digest = crate::setup_recovery::value_digest(&original)?;
                if proof.intent_digest != digest || ready_proof.intent_digest != digest {
                    return Err(refused());
                }
                (StorageControlOwner::Activation, original.storage_system.evidence(),
                    (*original.config).clone(), intent, ready, complete)
            }
        };
        Ok(StorageRecoveryPredecessor::Initialization { evidence: StorageOwnerEvidence {
            owner, binding, configuration, intent: evidence(&intent),
            ready: evidence(&ready), complete: evidence(&complete),
        } })
    }

    pub(in crate::setup_recovery) fn require_recovery_predecessor(
        &self, expected: &StorageRecoveryPredecessor,
    ) -> Result<(), ApiError> {
        self.control_directory.verify_identity().map_err(control_publication_error)?;
        let observed = self.capture_recovery_predecessor()?;
        if storage_contract::canonical_json(&observed).map_err(ApiError::invalid_source)?
            != storage_contract::canonical_json(expected).map_err(ApiError::invalid_source)?
        { return Err(refused()); }
        Ok(())
    }

    pub(in crate::setup_recovery) fn recovery_control_admitted(&self) -> Result<bool, ApiError> {
        if !activation::present(&self.data_root.join(RECOVERY_INTENT))? { return Ok(false); }
        let intent: StorageRecoveryIntent = self.read_durable_control(RECOVERY_INTENT)?;
        self.require_recovery_predecessor(&intent.control.predecessor)?;
        let system = StorageSystemBinding::from_evidence(&intent.control.binding).open(&self.data_root)?;
        Engine::probe_recovery_control(&system, &intent.control).map_err(engine_error)?;
        Ok(true)
    }

    pub(crate) fn require_recovered_engine(&self, engine: &Engine) -> Result<(), ApiError> {
        if !activation::present(&self.data_root.join(RECOVERY_INTENT))? {
            return engine.require_recovery_control(None).map_err(engine_error);
        }
        let intent: StorageRecoveryIntent = self.read_durable_control(RECOVERY_INTENT)?;
        self.require_recovery_predecessor(&intent.control.predecessor)?;
        let system = StorageSystemBinding::from_evidence(&intent.control.binding).open(&self.data_root)?;
        if system.path() != engine.root() { return Err(refused()); }
        engine.require_recovery_control(Some(&intent.control)).map_err(engine_error)
    }
}
