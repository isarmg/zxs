//! Build all empty-pool inputs from the same proved initialization occurrence
//! before installing any scope objects. No local descriptor is a vote.
use crate::disk::{DiskResources, DiskRoot};
use crate::engine::{
    disk_catalog::genesis::GenesisDiskImages,
    index::preparation::SystemEncodingInput,
    scope_closure::{
        PreparedImage, Schema, ScopeClosureError,
        objects::{Domain, PreparedObject},
        schema::{
            Closure, Counts, DiskManifest, Material, Membership, Origin, Scope, SystemMaterial,
            Totals,
        },
    },
};
use crate::history_package::AdmittedState;
use crate::journal::PreparedMetadataObject;
use crate::metadata_map::{Encoder, ProofMeasure, Reference};
use crate::{Result, State, ValidatedConfig};
use data_serialization::Serialize;
use std::{borrow::Cow, collections::BTreeMap};
use storage_contract::working_tree::{DiskRegistrationIdentity, MaterialCommitment, RootBinding};
use storage_contract::{DiskId, Generation, OperationId, PoolId, Sequence};

const PREPARATION_LIMIT: usize = 64 * 1024 * 1024;
pub(super) const MAX_DISKS: usize = 32;
pub(super) const PAGE_LIMIT: usize = 64 * 1024;

pub(in crate::engine) fn initial_state() -> State {
    State {
        topology_generation: 1,
        ..State::default()
    }
}

pub(in crate::engine) fn registration_key(identity: &DiskRegistrationIdentity) -> Result<String> {
    // The contract's full registration tuple is the map identity. Neither
    // disk ID alone nor a reusable slot is its lookup/routing key.
    String::from_utf8(storage_contract::canonical_json(identity)?)
        .map_err(|_| ScopeClosureError::GenesisIdentity.into())
}

struct DiskMaterial {
    manifest: PreparedObject,
    images: GenesisDiskImages,
}

pub(in crate::engine) struct GenesisPreparation {
    operation: OperationId,
    disks: BTreeMap<DiskId, DiskMaterial>,
    pages: Vec<PreparedMetadataObject>,
    empty_system_inventory: PreparedObject,
    system: PreparedObject,
    system_image: PreparedImage,
    state: AdmittedState,
    closure: PreparedObject,
}

impl GenesisPreparation {
    pub(in crate::engine) fn capture(
        config: &ValidatedConfig,
        operation: OperationId,
    ) -> Result<Self> {
        super::require_disk_budget(config)?;
        let state = AdmittedState::admit_replayed(initial_state())?;
        let pool_id = config.pool_id.parse::<PoolId>()?;
        let configuration_hash = MaterialCommitment::from_digest(
            storage_contract::canonical_object_digest("pool-config", &**config)?,
        );
        let origin = Origin::Genesis {
            initialization_operation_id: operation,
            config_hash: configuration_hash,
        };
        let mut pages = Encoder::new(PAGE_LIMIT);
        let empty = pages
            .map_root(json_codec::Map::new(), ProofMeasure::Plain)?
            .reference()
            .clone();
        let empty_system_inventory = PreparedObject::new(
            Domain::EmptyInventoryPage,
            &json_codec::Map::<String, json_codec::Value>::new(),
        )?;
        let (system, system_image, system_bytes) =
            prepare_system(pool_id, configuration_hash, &empty, &state)?;
        let mut total = Totals {
            disks: config.storage_roots.len() as u64,
            materials: 1,
            body_entries: 0,
            inventory_entries: 0,
            canonical_metadata_bytes: system_bytes as u64,
        };
        let mut used = system.bytes() + system_image.bytes() + empty_system_inventory.bytes();
        let mut disks = BTreeMap::new();
        let mut keys = BTreeMap::new();
        for (id, binding) in &config.storage_roots {
            let (disk, counts) = prepare_disk(binding, origin, &empty)?;
            add_bytes(&mut used, disk.images.bytes())?;
            add_bytes(&mut used, disk.manifest.bytes())?;
            total.materials += counts.materials;
            total.canonical_metadata_bytes += counts.canonical_metadata_bytes;
            keys.insert(
                registration_key(&binding.registration)?,
                json_codec::to_value(disk.manifest.hash())?,
            );
            disks.insert(*id, disk);
        }
        let registrations = pages.map_typed(keys, ProofMeasure::Plain, &|key: &String| {
            Cow::Borrowed(key.as_str())
        })?;
        let closure = PreparedObject::new(
            Domain::Closure,
            &Closure {
                pool_id,
                origin,
                sequence: Sequence::new(0),
                membership: Membership::Genesis {
                    config_hash: configuration_hash,
                },
                system_material: system.hash(),
                disks: registrations.reference().clone(),
                counts: total,
            },
        )?;
        let pages = pages.finish();
        for page in &pages {
            add_bytes(&mut used, page.bytes().len())?;
        }
        add_bytes(&mut used, closure.bytes())?;
        Ok(Self {
            operation,
            disks,
            pages,
            empty_system_inventory,
            system,
            system_image,
            state,
            closure,
        })
    }

    pub(in crate::engine) fn hash(&self) -> MaterialCommitment {
        self.closure.hash()
    }

    /// Write a holder's own image pair and manifest. Shared registration pages
    /// contain only identities and commitments. The SYSTEM image is installed
    /// once on SYSTEM, not on a data-only target or another member.
    pub(in crate::engine) fn install(
        &self,
        system_root: &DiskRoot,
        config: &ValidatedConfig,
        resources: &DiskResources,
    ) -> Result<()> {
        self.install_system_material(system_root)?;
        let original = super::readback::Original::new(config, self.operation, self.hash())?;
        let (mut actual_bytes, expected_bytes) = original.system(system_root)?;
        for disk in &config.disks {
            let id = disk.disk_id.parse::<DiskId>()?;
            let local = self
                .disks
                .get(&id)
                .ok_or(ScopeClosureError::GenesisIdentity)?;
            let held = &resources.get(disk)?.root;
            local.images.install(held)?;
            local.manifest.install(held)?;
            for page in &self.pages {
                page.store_at_root(held, disk)?;
            }
            self.closure.install(held)?;
            let actual = original.disk(disk, held)?;
            actual_bytes = actual_bytes
                .checked_add(actual.canonical_metadata_bytes)
                .ok_or(ScopeClosureError::Budget)?;
        }
        if actual_bytes != expected_bytes {
            return Err(ScopeClosureError::GenesisMismatch.into());
        }
        Ok(())
    }
    fn install_system_material(&self, system_root: &DiskRoot) -> Result<()> {
        let system_input = SystemEncodingInput::new(&self.state, &|| Ok(()))?;
        self.system_image
            .install_verified(system_root, |bytes| system_input.verify(bytes, &|| Ok(())))?;
        self.empty_system_inventory.install(system_root)?;
        self.system.install(system_root)?;
        self.closure.install(system_root)?;
        Ok(())
    }

    /// Explicit multi-voter restore only: material for a different, known
    /// empty SYSTEM target is admissible only if reproducing the entire fixed
    /// original graph gives the exact already-voted anchor bytes' commitment.
    pub(in crate::engine) fn prepare_system_restore(config: &ValidatedConfig) -> Result<Self> {
        let fixed = config.fixed_genesis()?;
        let original = super::Anchor::decode(fixed.bytes())?;
        let prepared = Self::capture(&original, fixed.operation)?;
        if prepared.hash() != fixed.closure {
            return Err(ScopeClosureError::ImageMismatch.into());
        }
        Ok(prepared)
    }
    pub(in crate::engine) fn install_restored_system(
        &self,
        system: &DiskRoot,
        config: &ValidatedConfig,
    ) -> Result<()> {
        if self.hash() != config.fixed_genesis()?.closure
            || system.entries(system.path())?.next().transpose()?.is_some()
        {
            return Err(ScopeClosureError::GenesisMismatch.into());
        }
        self.install_system_material(system)?;
        super::Anchor::verify_system(config, system)
    }
}

fn add_bytes(total: &mut usize, bytes: usize) -> Result<()> {
    *total = total
        .checked_add(bytes)
        .filter(|n| *n <= PREPARATION_LIMIT)
        .ok_or(ScopeClosureError::Budget)?;
    Ok(())
}

fn prepare_disk(
    binding: &RootBinding,
    origin: Origin,
    empty: &Reference,
) -> Result<(DiskMaterial, Counts)> {
    let identity = &binding.registration;
    let images = GenesisDiskImages::prepare(identity.clone())?;
    let material = |scope, (logical_root, representation)| Material {
        scope,
        logical_root,
        representation,
        identity: identity.clone(),
        body_inventory: empty.clone(),
    };
    let disk_current = material(Scope::DiskCurrent, images.current()?);
    let disk_history = material(Scope::DiskHistory, images.history()?);
    let bytes = storage_contract::canonical_json(&disk_current)?.len()
        + storage_contract::canonical_json(&disk_history)?.len();
    let counts = Counts {
        inventory_entries: 0,
        materials: 2,
        body_entries: 0,
        canonical_metadata_bytes: bytes as u64,
    };
    let manifest = PreparedObject::new(
        Domain::Disk,
        &DiskManifest {
            registration: identity.clone(),
            root_binding: binding.clone(),
            origin,
            sequence: Sequence::new(0),
            inventory: empty.clone(),
            disk_current,
            disk_history,
            local_changes: empty.clone(),
            retirement_obligations: empty.clone(),
            counts,
        },
    )?;
    Ok((DiskMaterial { manifest, images }, counts))
}

fn prepare_system(
    pool_id: PoolId,
    configuration_hash: MaterialCommitment,
    empty: &Reference,
    state: &AdmittedState,
) -> Result<(PreparedObject, PreparedImage, usize)> {
    let input = SystemEncodingInput::new(state, &|| Ok(()))?;
    let image = PreparedImage::new(Schema::System, input.encode(&|| Ok(()))?);
    let logical_root = system_logical_root()?;
    let material = Material {
        scope: Scope::System,
        identity: pool_id,
        logical_root,
        representation: image.representation(),
        body_inventory: empty.clone(),
    };
    let bytes = storage_contract::canonical_json(&material)?.len();
    let object = PreparedObject::new(
        Domain::System,
        &SystemMaterial {
            pool_id,
            sequence: Sequence::new(0),
            business_generation: Generation::new(state.state().business_generation),
            topology_generation: Generation::new(state.state().topology_generation),
            configuration_hash,
            material,
        },
    )?;
    Ok((object, image, bytes))
}

pub(super) fn system_logical_root() -> Result<MaterialCommitment> {
    // These are all logical rows of the genesis SYSTEM projection. Reading
    // the installed image independently verifies each table against the same
    // admitted empty State, not against a hash of raw State/control_auth.
    #[derive(Serialize)]
    struct EmptySystemRows {
        sequence: Sequence,
        history_versions: [(); 0],
        scope_members: [(); 0],
        system_changes: [(); 0],
    }
    Ok(MaterialCommitment::from_digest(
        storage_contract::canonical_object_digest(
            "system-scope-rows",
            &EmptySystemRows {
                sequence: Sequence::new(0),
                history_versions: [],
                scope_members: [],
                system_changes: [],
            },
        )?,
    ))
}
