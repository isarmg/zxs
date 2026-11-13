//! Decision-bound directory material syntax, never authority from local JSON.
//! A consumer must additionally admit the original SYSTEM decision and its DISK
//! proof before reopening material or exposing a registered directory.
use crate::working_tree::snapshot::SnapshotNodeBinding;
use data_serialization::{Deserialize, Serialize};
use storage_contract::working_tree::{
    LocalEntityIdentity, MaterialCommitment, RootBinding, SnapshotBranchId,
    SnapshotNodeId, SnapshotRepositoryId,
};
use storage_contract::OperationId;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct DirectoryInitializationBinding {
    pub(crate) intent: MaterialCommitment,
    pub(crate) operation: OperationId,
    pub(crate) root: RootBinding,
    pub(crate) host: LocalEntityIdentity,
    pub(crate) repository: SnapshotRepositoryId,
    pub(crate) branch: SnapshotBranchId,
    pub(crate) node: SnapshotNodeId,
    pub(crate) material: SnapshotNodeBinding,
    pub(crate) registration: MaterialCommitment,
}

#[derive(Debug, error_definition::Error)]
#[error("directory initialization material does not close over its decision")]
pub(crate) struct DirectoryBindingError;

impl DirectoryInitializationBinding {
    /// Structural closure only. Neither this method nor decoding grants access.
    pub(crate) fn validate(
        &self,
        operation: OperationId,
        pool_id: &str,
    ) -> Result<(), DirectoryBindingError> {
        let reference = self.material.reference();
        if self.operation != operation
            || self.root.registration.pool_id != pool_id
            || pool_id.is_empty()
            || self.root.registration.disk_id.is_empty()
            || self.root.registration.slot_id.is_empty()
            || self.root.registration.binding_generation == 0
            || !self.root.source_chain.is_empty()
            || self.host.device() != self.root.root_entity.device()
            || reference.repository != self.repository
            || reference.node != self.node
        {
            return Err(DirectoryBindingError);
        }
        Ok(())
    }
}

pub(super) const INITIALIZE_KIND: &str = "directory_snapshot_initialize";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct RegistrationRecord {
    pub(super) intent: super::intent::SnapshotInitIntent,
    pub(super) material: SnapshotNodeBinding,
}

#[derive(Debug, error_definition::Error)]
pub(super) enum RegistrationError {
    #[error("encoding directory initialization registration")]
    Encode(#[source] storage_contract::Error),
    #[error("decoding directory initialization registration")]
    Decode(#[source] storage_contract::Error),
    #[error("directory initialization registration retains an invalid original intent")]
    Intent(#[source] super::intent::IntentError),
    #[error("directory initialization registration disagrees with its original intent")]
    Material,
    #[error("directory initialization registration disagrees with its permanent candidate")]
    Candidate,
}

fn commitment(domain: &str, bytes: &[u8]) -> MaterialCommitment {
    let mut hasher = blake3::Hasher::new_derive_key(domain);
    hasher.update(bytes);
    MaterialCommitment::from_digest(hasher.finalize())
}

impl RegistrationRecord {
    pub(super) fn prepare(
        intent: &super::intent::AdmittedIntent,
        material: SnapshotNodeBinding,
    ) -> Result<(Self, DirectoryInitializationBinding), RegistrationError> {
        if material.reference() != intent.record().material {
            return Err(RegistrationError::Material);
        }
        let record = Self { intent: intent.record().clone(), material };
        let binding = record.binding()?;
        Ok((record, binding))
    }

    fn binding(&self) -> Result<DirectoryInitializationBinding, RegistrationError> {
        let intent_bytes = storage_contract::canonical_json(&self.intent)
            .map_err(RegistrationError::Encode)?;
        let intent = super::intent::AdmittedIntent::decode(&intent_bytes)
            .map_err(RegistrationError::Intent)?;
        if self.material.reference() != intent.record().material {
            return Err(RegistrationError::Material);
        }
        let bytes = storage_contract::canonical_json(self)
            .map_err(RegistrationError::Encode)?;
        Ok(DirectoryInitializationBinding {
            intent: commitment("zxs directory initialization intent v1", &intent_bytes),
            operation: self.intent.operation,
            root: self.intent.root.clone(),
            host: self.intent.host_entity,
            repository: self.intent.repository,
            branch: self.intent.branch,
            node: self.intent.node,
            material: self.material,
            registration: commitment("zxs directory initialization registration v1", &bytes),
        })
    }

    pub(super) fn result_value(&self) -> Result<json_codec::Value, RegistrationError> {
        let bytes = storage_contract::canonical_json(self)
            .map_err(RegistrationError::Encode)?;
        storage_contract::decode_strict(&bytes).map_err(RegistrationError::Decode)
    }

    /// Checks a result from the independently hash-admitted original snapshot.
    /// This establishes the typed cross-record binding only. Callers still must
    /// verify FINALIZE/publication, current disk registration and complete held
    /// physical material; a matching JSON result is never a read capability.
    pub(super) fn from_candidate_result(
        candidate: &crate::journal::Candidate,
        result: &crate::OperationResult,
    ) -> Result<Self, RegistrationError> {
        let binding = candidate.directory_material().ok_or(RegistrationError::Candidate)?;
        if result.kind != INITIALIZE_KIND
            || result.operation_id != candidate.operation_id
            || result.sequence != candidate.sequence
            || result.request_digest != candidate.request_digest
            || result.commit_id.is_some()
            || result.history_tree_id.is_some()
            || result.branch_id.is_some()
        {
            return Err(RegistrationError::Candidate);
        }
        let bytes = storage_contract::canonical_json(&result.result)
            .map_err(RegistrationError::Encode)?;
        let record: Self = storage_contract::decode_strict(&bytes)
            .map_err(RegistrationError::Decode)?;
        if &record.binding()? != binding
            || candidate.request_digest != binding.intent.hash().to_string()
            || candidate.previous_snapshot.as_deref()
                != Some(record.intent.source.publication.hash().to_string().as_str())
            || record.intent.source.sequence.get() >= candidate.sequence
        {
            return Err(RegistrationError::Candidate);
        }
        Ok(record)
    }
}
