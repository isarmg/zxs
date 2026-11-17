//! The sole fixed-address bytes. Ordinary Config remains the single unchanged
//! runtime projection and cannot be deserialized as this fixed anchor.
use super::{ScopeClosureError as Failure, genesis_operation, readback::Original};
use crate::{
    Config, DiskConfig, Result, ValidatedConfig,
    disk::{DiskRoot, MAX_METADATA},
    persist::layout,
};
use data_serialization::{Deserialize, Serialize};
use std::sync::Arc;
use storage_contract::OperationId;
use storage_contract::working_tree::MaterialCommitment;

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct FixedGenesis {
    config: Config,
    initialization_operation_id: OperationId,
    scope_closure: MaterialCommitment,
}

/// Read-only presentation of the exact immutable original. This private
/// witness is never serialized as part of Config or a successor authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::engine) struct Anchor {
    pub(in crate::engine) operation: OperationId,
    pub(in crate::engine) closure: MaterialCommitment,
    pub(in crate::engine) hash: MaterialCommitment,
    bytes: Arc<[u8]>,
}
impl Anchor {
    pub(in crate::engine) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(in crate::engine) fn initial(
        config: &ValidatedConfig,
        operation: OperationId,
        scope_closure: MaterialCommitment,
    ) -> Result<ValidatedConfig> {
        // Preserve restart-only in-memory limits and never leak them into the
        // original Config commitment.
        if genesis_operation(config)? != operation {
            return Err(Failure::GenesisIdentity.into());
        }
        let dto = FixedGenesis {
            config: (**config).clone(),
            initialization_operation_id: operation,
            scope_closure,
        };
        let (bytes, hash) =
            storage_contract::encode_canonical_object_digest("fixed-genesis", &dto)?;
        if bytes.len() > MAX_METADATA {
            return Err(Failure::Budget.into());
        }
        Ok(config.clone().with_fixed_genesis(Self {
            operation,
            closure: scope_closure,
            hash: MaterialCommitment::from_digest(hash),
            bytes: bytes.into(),
        }))
    }

    pub(in crate::engine) fn decode(bytes: &[u8]) -> Result<ValidatedConfig> {
        if bytes.len() > MAX_METADATA {
            return Err(Failure::Budget.into());
        }
        let dto: FixedGenesis = storage_contract::decode_strict(bytes)?;
        let (canonical, hash) =
            storage_contract::encode_canonical_object_digest("fixed-genesis", &dto)?;
        if canonical != bytes || genesis_operation(&dto.config)? != dto.initialization_operation_id
        {
            return Err(Failure::GenesisMismatch.into());
        }
        let config = ValidatedConfig::try_from(dto.config)?;
        super::require_disk_budget(&config)?;
        crate::engine::fresh_layout::require_persisted_roots(&config)?;
        Ok(config.with_fixed_genesis(Self {
            operation: dto.initialization_operation_id,
            closure: dto.scope_closure,
            hash: MaterialCommitment::from_digest(hash),
            bytes: canonical.into(),
        }))
    }

    pub(in crate::engine) fn read(root: &DiskRoot) -> Result<ValidatedConfig> {
        let file = root
            .open_optional_read_relative(&layout::GENESIS_CONFIGURATION)?
            .ok_or(Failure::GenesisMismatch)?;
        Self::decode(&crate::disk::read_file_bytes(file, MAX_METADATA)?)
    }

    pub(in crate::engine) fn verify_managed_recognition(
        config: &ValidatedConfig,
        disk: &DiskConfig,
        root: &DiskRoot,
    ) -> Result<()> {
        let fixed = config.fixed_genesis()?;
        let original = Self::decode(fixed.bytes())?;
        Self::verify_if_original(&original, disk, root)
    }

    pub(in crate::engine) fn verify_system(
        config: &ValidatedConfig,
        root: &DiskRoot,
    ) -> Result<()> {
        let fixed = config.fixed_genesis()?;
        let original = Self::decode(fixed.bytes())?;
        Original::new(&original, fixed.operation, fixed.closure)?.system(root)?;
        Ok(())
    }

    /// All named, actually available holders must match the independent
    /// original witness; unavailable or stopped holders are never probed anew.
    /// A successor registration is verified by its native root admission, not
    /// silently promoted to an original inventory entry.
    pub(in crate::engine) fn preflight_holders(
        config: &ValidatedConfig,
        resources: &crate::disk::DiskResources,
    ) -> Result<()> {
        let fixed = config.fixed_genesis()?;
        for disk in config.io_disks() {
            let Some(handle) = resources.available(disk) else {
                continue;
            };
            crate::engine::fresh_layout::verify_root(config, disk, &handle.root)?;
            let local = Self::read(&handle.root)?;
            if local.fixed_genesis()?.hash != fixed.hash {
                return Err(Failure::GenesisMismatch.into());
            }
            if config.generation().is_some() {
                Self::verify_if_original(&local, disk, &handle.root)?;
            } else {
                Self::verify_original_disk(&local, disk, &handle.root)?;
            }
        }
        Ok(())
    }
    /// Read one real original registration against this exact saved genesis.
    /// A successor's added registration is *not* an original local witness.
    pub(in crate::engine) fn verify_original_disk(
        config: &ValidatedConfig,
        actual: &DiskConfig,
        root: &DiskRoot,
    ) -> Result<()> {
        let witness = config.fixed_genesis()?;
        Original::new(config, witness.operation, witness.closure)?.disk(actual, root)?;
        Ok(())
    }

    pub(in crate::engine) fn verify_if_original(
        config: &ValidatedConfig,
        actual: &DiskConfig,
        root: &DiskRoot,
    ) -> Result<()> {
        if let Some(original) = config.disks.iter().find(|d| {
            d.disk_id == actual.disk_id
                && d.slot_id == actual.slot_id
                && d.binding_generation == actual.binding_generation
        }) {
            let mut local = original.clone();
            local.root = root.path().to_owned();
            Self::verify_original_disk(config, &local, root)?;
        }
        Ok(())
    }
}
