//! Readback of an actual initializer result, never a configuration-only grant.
use super::{Engine, ValidatedConfig, fresh_layout};
use crate::{
    Result,
    disk::{DiskResources, MAX_METADATA, decode_seal, validate_stored_seal},
    persist::{initial_storage, layout},
};
use std::collections::BTreeMap;
use storage_contract::{DiskId, working_tree::LocalEntityIdentity};

pub use crate::persist::initial_storage::InitialSealDigest;
use super::disk_lifecycle::bootstrap::{BootstrapError, runtime::management_error};

/// Observed original disk identity and its existing activation seal digest.
/// This is read-only evidence, not permission to open or write a disk.
pub struct InitialStorageDisk {
    root: LocalEntityIdentity,
    seal: InitialSealDigest,
}
impl InitialStorageDisk {
    pub fn root(&self) -> LocalEntityIdentity {
        self.root
    }

    pub fn seal(&self) -> &InitialSealDigest {
        &self.seal
    }
}

/// Produced only by readback through the actual initialization/recovery handles.
/// The original unbound request remains immutable in the Server's intent.
pub struct InitialStorageObservation {
    disks: BTreeMap<DiskId, InitialStorageDisk>,
}
impl InitialStorageObservation {
    pub fn disks(&self) -> &BTreeMap<DiskId, InitialStorageDisk> {
        &self.disks
    }
}

pub(super) fn observe(
    input: &ValidatedConfig,
    config: &ValidatedConfig,
    resources: &DiskResources,
) -> Result<InitialStorageObservation> {
    if input.generation().is_some()
        || config.generation().is_some()
        || !config.is_initialization_result_of(input)
    {
        return Err(management_error(BootstrapError::ManagementOnly(
            "storage completion does not match its original initialization request",
        )));
    }
    let expected = storage_contract::canonical_json(config)?;
    let mut disks = BTreeMap::new();
    for disk in &config.disks {
        // This requires every original root, not an available subset/quorum.
        // The same retained handles verify all five created area identities.
        let held = resources.get(disk)?;
        fresh_layout::verify_root(config, disk, &held.root)?;
        let address = disk.root.join(layout::GENESIS_CONFIGURATION.as_path());
        let bytes = held.root_for(&address)?.read_bytes(&address, MAX_METADATA)?;
        let genesis: crate::Config = storage_contract::decode_strict(&bytes).map_err(|source| {
            management_error(BootstrapError::Encoding {
                stage: "read initial fixed genesis",
                source,
            })
        })?;
        if storage_contract::canonical_json(&genesis)? != expected {
            return Err(management_error(BootstrapError::ManagementOnly(
                "initial storage differs from its fixed genesis configuration",
            )));
        }
        let bytes = held.root.read_entry_bytes(&layout::DISK_IDENTITY, MAX_METADATA)?;
        let seal = decode_seal(&bytes)?;
        validate_stored_seal(&seal, &held.seal)?;
        disks.insert(disk.disk_id.parse::<DiskId>()?, InitialStorageDisk {
            root: LocalEntityIdentity::new(seal.device, seal.inode),
            seal: initial_storage::seal_digest(&bytes)?,
        });
    }
    Ok(InitialStorageObservation { disks })
}

impl Engine {
    /// Read back the actual created roots and fixed Genesis for an original
    /// initialization request. No path or configuration supplied by the caller
    /// becomes a resource; only this Engine's already-held resources are read.
    ///
    /// # Errors
    /// Rejects any changed request, successor epoch, unavailable original disk,
    /// changed root/area entity, seal, or fixed Genesis configuration.
    pub fn initial_storage_observation(
        &self,
        input: &ValidatedConfig,
    ) -> Result<InitialStorageObservation> {
        observe(input, &self.config, &self.disk_resources)
    }
}
