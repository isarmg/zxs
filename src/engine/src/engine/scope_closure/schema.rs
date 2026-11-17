//! Canonical fields of the scope graph. Binary SQLite data is referenced,
//! never embedded in a registration page or sent to every holder.
use crate::engine::scope_closure::Representation;
use crate::metadata_map::Reference;
use data_serialization::{Deserialize, Serialize};
use storage_contract::working_tree::{DiskRegistrationIdentity, MaterialCommitment, RootBinding};
use storage_contract::{Generation, OperationId, PoolId, Sequence};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Origin {
    Genesis {
        initialization_operation_id: OperationId,
        config_hash: MaterialCommitment,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Membership {
    Genesis { config_hash: MaterialCommitment },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub(super) enum Scope {
    DiskCurrent,
    DiskHistory,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Material<I> {
    pub scope: Scope,
    pub identity: I,
    pub logical_root: MaterialCommitment,
    pub representation: Representation,
    pub body_inventory: Reference,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Counts {
    #[serialization(with = "storage_contract::u64_string")]
    pub inventory_entries: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub materials: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub body_entries: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub canonical_metadata_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Totals {
    #[serialization(with = "storage_contract::u64_string")]
    pub disks: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub inventory_entries: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub materials: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub body_entries: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub canonical_metadata_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct DiskManifest {
    pub registration: DiskRegistrationIdentity,
    pub root_binding: RootBinding,
    pub origin: Origin,
    pub sequence: Sequence,
    pub inventory: Reference,
    pub disk_current: Material<DiskRegistrationIdentity>,
    pub disk_history: Material<DiskRegistrationIdentity>,
    pub local_changes: Reference,
    pub retirement_obligations: Reference,
    pub counts: Counts,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct SystemMaterial {
    pub pool_id: PoolId,
    pub sequence: Sequence,
    pub business_generation: Generation,
    pub topology_generation: Generation,
    pub configuration_hash: MaterialCommitment,
    pub material: Material<PoolId>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Closure {
    pub pool_id: PoolId,
    pub origin: Origin,
    pub sequence: Sequence,
    pub membership: Membership,
    pub system_material: MaterialCommitment,
    pub disks: Reference,
    pub counts: Totals,
}
