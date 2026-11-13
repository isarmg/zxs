//! Typed control lineage carried by the sole recovery selector. Historical
//! member decisions keep their original schema and bind this evidence by hash.
use crate::authority_selection::FreshSelection;
use crate::Config;
use data_serialization::{Deserialize, Serialize};
use storage_contract::{OperationId, Principal, working_tree::{LocalEntityIdentity, MaterialCommitment}};

/// A trusted-local inventory selection label, not a persistent storage DiskId.
/// Its exact text remains part of the explicit recovery control commitment.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct RecoveryDiskSelection(Box<str>);
impl RecoveryDiskSelection {
    pub fn parse(value: &str) -> crate::Result<Self> {
        if value.is_empty() || value.len() > 64
            || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(crate::Error::RecoveryDiskSelection(
                crate::InvalidRecoveryDiskSelection));
        }
        Ok(Self(value.into()))
    }
}
impl Serialize for RecoveryDiskSelection {
    fn serialize<S: data_serialization::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for RecoveryDiskSelection {
    fn deserialize<D: data_serialization::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(data_serialization::de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct StorageControlBinding {
    pub operation: OperationId,
    pub control: LocalEntityIdentity,
    pub system: LocalEntityIdentity,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum StorageControlOwner { Setup, Activation }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct StorageOwnerEvidence {
    pub owner: StorageControlOwner,
    pub binding: StorageControlBinding,
    pub configuration: Config,
    pub intent: MaterialCommitment,
    pub ready: MaterialCommitment,
    pub complete: MaterialCommitment,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StorageRecoveryPredecessor {
    NewControl,
    Initialization { evidence: StorageOwnerEvidence },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "bound-control-recovery-v1")]
    BoundControl,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct StorageRecoveryControl {
    format: Format,
    pub binding: StorageControlBinding,
    pub source_evidence: MaterialCommitment,
    pub principal: Principal,
    pub disk_ids: Vec<RecoveryDiskSelection>,
    pub predecessor: StorageRecoveryPredecessor,
}

impl StorageRecoveryControl {
    pub fn new(
        binding: StorageControlBinding, source_evidence: MaterialCommitment,
        principal: Principal, disk_ids: Vec<RecoveryDiskSelection>, predecessor: StorageRecoveryPredecessor,
    ) -> Self {
        Self { format: Format::BoundControl, binding, source_evidence, principal, disk_ids, predecessor }
    }

    pub fn commitment(&self) -> crate::Result<MaterialCommitment> {
        let bytes = storage_contract::canonical_json(self)?;
        let mut hash = blake3::Hasher::new_derive_key("zxs bound storage recovery control v1");
        hash.update(&bytes);
        Ok(MaterialCommitment::from_digest(hash.finalize()))
    }

    pub(crate) fn require_origin(&self, origin: &FreshSelection) -> crate::Result<()> {
        if self.disk_ids.is_empty() || self.disk_ids.len() > 32
            || self.disk_ids.iter().collect::<std::collections::BTreeSet<_>>().len()
                != self.disk_ids.len()
            || self.binding.system == self.binding.control
            || self.binding.system == origin.system
            || self.binding.operation == origin.operation
        {
            return Err(super::refused());
        }
        if let StorageRecoveryPredecessor::Initialization { evidence } = &self.predecessor {
            if evidence.binding.control != self.binding.control
                || evidence.binding.system != origin.system
                || evidence.binding.operation != origin.operation
            {
                return Err(super::refused());
            }
            let config = crate::ValidatedConfig::try_from(evidence.configuration.clone())?;
            origin.require_config(&config)?;
        }
        Ok(())
    }
}
