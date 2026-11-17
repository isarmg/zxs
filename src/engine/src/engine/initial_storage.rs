//! Readback of an actual initializer result, never a configuration-only grant.
use super::{Engine, ValidatedConfig, fresh_layout};
use crate::{
    Result,
    disk::{DiskResources, MAX_METADATA, decode_seal, validate_stored_seal},
    persist::{initial_storage, layout},
};
use std::collections::BTreeMap;
use storage_contract::{DiskId, working_tree::LocalEntityIdentity};

use super::disk_lifecycle::bootstrap::{BootstrapError, runtime::management_error};
pub use crate::persist::initial_storage::InitialSealDigest;

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
    // Complete independently read registration set, not a writer or disk FD.
    original: ValidatedConfig,
}
impl InitialStorageObservation {
    pub fn disks(&self) -> &BTreeMap<DiskId, InitialStorageDisk> {
        &self.disks
    }

    /// Recheck this one original disk at the previously proved fixed genesis.
    /// No other disk or SYSTEM is read. This neither selects a newer frontier
    /// nor proves the absence of newer publications or retired contents.
    ///
    /// # Errors
    /// Refuses unregistered, rebound, resealed or damaged input; the caller's
    /// explicit directory handle cannot be replaced by a stored disk path.
    pub fn verify_initial_disk_scope(
        &self,
        id: DiskId,
        address: &std::path::Path,
        directory: std::fs::File,
    ) -> Result<storage_contract::working_tree::MaterialCommitment> {
        let fixed = self.original.fixed_genesis()?;
        let observed = self
            .disks
            .get(&id)
            .ok_or(super::scope_closure::ScopeClosureError::GenesisIdentity)?;
        let original_id = id.to_string();
        let original = self
            .original
            .disks
            .iter()
            .find(|d| d.disk_id == original_id)
            .ok_or(super::scope_closure::ScopeClosureError::GenesisIdentity)?;
        let mut local = original.clone();
        let root = crate::disk::DiskRoot::from_directory(address.to_owned(), directory)?;
        local.root = root.path().to_owned();
        let bytes = root.read_entry_bytes(&layout::DISK_IDENTITY, MAX_METADATA)?;
        let seal = decode_seal(&bytes)?;
        validate_stored_seal(
            &seal,
            &crate::disk::identity_at(&self.original, &local, &root.directory(root.path())?)?,
        )?;
        if initial_storage::seal_digest(&bytes)? != observed.seal
            || LocalEntityIdentity::new(seal.device, seal.inode) != observed.root
        {
            return Err(super::scope_closure::ScopeClosureError::GenesisIdentity.into());
        }
        let saved = super::scope_closure::Anchor::read(&root)?;
        if saved.fixed_genesis()?.hash != fixed.hash {
            return Err(super::scope_closure::ScopeClosureError::GenesisMismatch.into());
        }
        super::scope_closure::Anchor::verify_original_disk(&self.original, &local, &root)?;
        Ok(fixed.closure)
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
    let original_operation = super::scope_closure::genesis_operation(config)?;
    let expected = storage_contract::canonical_json(config)?;
    let mut selected: Option<ValidatedConfig> = None;
    let mut disks = BTreeMap::new();
    for disk in &config.disks {
        // This requires every original root, not an available subset/quorum.
        // The same retained handles verify all five created area identities.
        let held = resources.get(disk)?;
        fresh_layout::verify_root(config, disk, &held.root)?;
        let address = disk.root.join(layout::GENESIS_CONFIGURATION.as_path());
        let bytes = held
            .root_for(&address)?
            .read_bytes(&address, MAX_METADATA)?;
        let genesis = super::scope_closure::Anchor::decode(&bytes)?;
        let witness = genesis.fixed_genesis()?;
        if witness.operation != original_operation
            || selected
                .as_ref()
                .map(|previous| previous.fixed_genesis().map(|w| w.hash))
                .transpose()?
                .is_some_and(|hash| hash != witness.hash)
            || storage_contract::canonical_json(&genesis)? != expected
        {
            return Err(management_error(BootstrapError::ManagementOnly(
                "initial storage differs from its fixed genesis configuration",
            )));
        }
        super::scope_closure::Anchor::verify_original_disk(&genesis, disk, &held.root)?;
        selected.get_or_insert(genesis);
        let bytes = held
            .root
            .read_entry_bytes(&layout::DISK_IDENTITY, MAX_METADATA)?;
        let seal = decode_seal(&bytes)?;
        validate_stored_seal(&seal, &held.seal)?;
        disks.insert(
            disk.disk_id.parse::<DiskId>()?,
            InitialStorageDisk {
                root: LocalEntityIdentity::new(seal.device, seal.inode),
                seal: initial_storage::seal_digest(&bytes)?,
            },
        );
    }
    let original = selected.ok_or(super::scope_closure::ScopeClosureError::GenesisIdentity)?;
    Ok(InitialStorageObservation { disks, original })
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
