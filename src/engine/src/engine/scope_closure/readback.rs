//! Admit only the immutable local graph selected by a caller's original
//! frontier. This module never chooses a frontier and never repairs material.
use super::{
    ScopeClosureError as Failure,
    genesis::{PAGE_LIMIT, registration_key},
    objects::{self, Domain},
    schema::{Closure, Counts, DiskManifest, Membership, Origin, Scope, SystemMaterial},
};
use crate::{DiskConfig, Result, ValidatedConfig, disk::DiskRoot, journal, metadata_map};
use storage_contract::working_tree::MaterialCommitment;
use storage_contract::{DiskId, Generation, OperationId, PoolId, Sequence};

pub(super) struct Original<'a> {
    config: &'a ValidatedConfig,
    pool: PoolId,
    origin: Origin,
    config_hash: MaterialCommitment,
    scope: MaterialCommitment,
}
impl<'a> Original<'a> {
    pub(super) fn new(
        config: &'a ValidatedConfig,
        operation: OperationId,
        scope: MaterialCommitment,
    ) -> Result<Self> {
        if super::genesis_operation(config)? != operation {
            return Err(Failure::GenesisIdentity.into());
        }
        let config_hash = MaterialCommitment::from_digest(
            storage_contract::canonical_object_digest("pool-config", &**config)?,
        );
        Ok(Self {
            config,
            pool: config.pool_id.parse()?,
            origin: Origin::Genesis {
                initialization_operation_id: operation,
                config_hash,
            },
            config_hash,
            scope,
        })
    }

    fn closure(&self, root: &DiskRoot) -> Result<Closure> {
        let closure: Closure = objects::load(root, Domain::Closure, self.scope)?;
        let n = self.config.storage_roots.len() as u64;
        if n == 0
            || n > 32
            || closure.pool_id != self.pool
            || closure.origin != self.origin
            || closure.sequence != Sequence::new(0)
            || closure.membership
                != (Membership::Genesis {
                    config_hash: self.config_hash,
                })
            || closure.counts.disks != n
            || closure.counts.materials != 2 * n + 1
            || closure.counts.body_entries != 0
            || closure.counts.inventory_entries != 0
        {
            return Err(Failure::GenesisMismatch.into());
        }
        Ok(closure)
    }

    /// The supplied DiskConfig provides the actual selected address; signed
    /// historical absolute paths never redirect I/O away from the held root.
    pub(super) fn disk(&self, disk: &DiskConfig, root: &DiskRoot) -> Result<Counts> {
        crate::engine::fresh_layout::verify_root(self.config, disk, root)?;
        let binding = self
            .config
            .storage_roots
            .get(&disk.disk_id.parse::<DiskId>()?)
            .ok_or(Failure::GenesisIdentity)?;
        let closure = self.closure(root)?;
        // The original pool holds at most 32 bounded registrations. Its single
        // canonical leaf exposes identities/hashes, not other members' bodies.
        if closure.disks.kind() != metadata_map::ReferenceKind::Page {
            return Err(Failure::GenesisMismatch.into());
        }
        let rows = metadata_map::read_map(&closure.disks, |domain, hash| {
            journal::load_scope_page_at(root, disk, domain, hash, PAGE_LIMIT)
        })?;
        if rows.len() != self.config.storage_roots.len() {
            return Err(Failure::GenesisMismatch.into());
        }
        for expected in self.config.storage_roots.values() {
            let row = rows
                .get(&registration_key(&expected.registration)?)
                .ok_or(Failure::GenesisMismatch)?;
            let _: MaterialCommitment =
                storage_contract::decode_strict(&storage_contract::canonical_json_value(row)?)?;
        }
        let row = rows
            .get(&registration_key(&binding.registration)?)
            .ok_or(Failure::GenesisMismatch)?;
        let commitment =
            storage_contract::decode_strict(&storage_contract::canonical_json_value(row)?)?;
        let manifest: DiskManifest = objects::load(root, Domain::Disk, commitment)?;
        let current = &manifest.disk_current;
        let history = &manifest.disk_history;
        if manifest.registration != binding.registration
            || manifest.root_binding != *binding
            || manifest.origin != self.origin
            || manifest.sequence != Sequence::new(0)
            || current.scope != Scope::DiskCurrent
            || history.scope != Scope::DiskHistory
            || current.identity != binding.registration
            || history.identity != binding.registration
            || current.body_inventory != manifest.inventory
            || history.body_inventory != manifest.inventory
            || manifest.local_changes != manifest.inventory
            || manifest.retirement_obligations != manifest.inventory
            || manifest.inventory.kind() != metadata_map::ReferenceKind::Page
        {
            return Err(Failure::GenesisMismatch.into());
        }
        let inventory = metadata_map::read_map(&manifest.inventory, |domain, hash| {
            journal::load_scope_page_at(root, disk, domain, hash, PAGE_LIMIT)
        })?;
        if !inventory.is_empty() {
            return Err(Failure::GenesisMismatch.into());
        }
        let counts = Counts {
            inventory_entries: 0,
            materials: 2,
            body_entries: 0,
            canonical_metadata_bytes: (storage_contract::canonical_json(current)?.len()
                + storage_contract::canonical_json(history)?.len())
                as u64,
        };
        if counts != manifest.counts {
            return Err(Failure::GenesisMismatch.into());
        }
        crate::engine::disk_catalog::genesis::verify_original(
            root,
            &binding.registration,
            current.logical_root,
            current.representation,
            history.logical_root,
            history.representation,
        )?;
        crate::engine::fresh_layout::verify_root(self.config, disk, root)?;
        Ok(counts)
    }

    /// SYSTEM is a separate holder. No voting/data disk must contain this
    /// descriptor or the image it names in order to prove its own local scope.
    pub(super) fn system(&self, root: &DiskRoot) -> Result<(u64, u64)> {
        let closure = self.closure(root)?;
        let system: SystemMaterial = objects::load(root, Domain::System, closure.system_material)?;
        let material = &system.material;
        let empty = metadata_map::Encoder::new(PAGE_LIMIT)
            .map_root(json_codec::Map::new(), metadata_map::ProofMeasure::Plain)?;
        if system.pool_id != self.pool
            || system.sequence != Sequence::new(0)
            || system.configuration_hash != self.config_hash
            || system.business_generation != Generation::new(0)
            || system.topology_generation != Generation::new(1)
            || material.scope != Scope::System
            || material.identity != self.pool
            || &material.body_inventory != empty.reference()
        {
            return Err(Failure::GenesisMismatch.into());
        }
        let empty_hash =
            MaterialCommitment::from_digest(storage_contract::canonical_object_digest(
                Domain::EmptyInventoryPage.name(),
                &json_codec::Map::<String, json_codec::Value>::new(),
            )?);
        let actual_empty: json_codec::Map<String, json_codec::Value> =
            objects::load(root, Domain::EmptyInventoryPage, empty_hash)?;
        if !actual_empty.is_empty() {
            return Err(Failure::GenesisMismatch.into());
        }
        let bytes = material.representation.read(
            root,
            super::Schema::System,
            crate::engine::index::preparation::MAX_BYTES,
        )?;
        let state = crate::history_package::AdmittedState::admit_replayed(super::initial_state())?;
        let actual =
            crate::engine::index::preparation::SystemEncodingInput::new(&state, &|| Ok(()))?
                .verified_scope_root(&bytes, &|| Ok(()))?;
        if actual != material.logical_root {
            return Err(Failure::GenesisMismatch.into());
        }
        Ok((
            storage_contract::canonical_json(material)?.len() as u64,
            closure.counts.canonical_metadata_bytes,
        ))
    }
}
