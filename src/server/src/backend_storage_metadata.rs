//! Request-owned storage metadata. Local disk paths are not part of the snapshot.
use data_serialization::Serialize;
use std::collections::BTreeMap;

#[derive(Debug)]
pub struct TopologySnapshot {
    pub generation: u64,
    pub slots: Vec<TopologySlot>,
}
#[derive(Debug)]
pub struct TopologySlot {
    pub slot_id: String,
    pub disk_id: String,
    pub level: u32,
    pub upstream_slot_id: Option<String>,
    pub metadata_member: bool,
    pub binding_generation: u64,
}
#[derive(Serialize)]
pub(crate) struct TopologyResponse {
    topology_id: &'static str,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
    slots: Vec<TopologySlotResponse>,
}
#[derive(Serialize)]
struct TopologySlotResponse {
    slot_id: String,
    disk_id: String,
    level: u32,
    upstream_slot_id: Option<String>,
    metadata_member: bool,
    #[serialization(with = "storage_contract::u64_string")]
    binding_generation: u64,
}
impl From<TopologySnapshot> for TopologyResponse {
    fn from(snapshot: TopologySnapshot) -> Self {
        let TopologySnapshot { generation, slots } = snapshot;
        Self {
            topology_id: "default",
            generation,
            slots: slots
                .into_iter()
                .map(|slot| {
                    let TopologySlot {
                        slot_id,
                        disk_id,
                        level,
                        upstream_slot_id,
                        metadata_member,
                        binding_generation,
                    } = slot;
                    TopologySlotResponse {
                        slot_id,
                        disk_id,
                        level,
                        upstream_slot_id,
                        metadata_member,
                        binding_generation,
                    }
                })
                .collect(),
        }
    }
}
#[derive(Serialize)]
pub(crate) struct PlacementResponse {
    version_id: String,
    primary_slot_id: String,
    target_copies: u32,
    route_slot_ids: Vec<String>,
    #[serialization(with = "storage_contract::u64_string")]
    topology_generation: u64,
    #[serialization(with = "storage_contract::u64_string_map")]
    binding_generations: BTreeMap<String, u64>,
}
impl From<storage_contract::Placement> for PlacementResponse {
    fn from(placement: storage_contract::Placement) -> Self {
        let storage_contract::Placement {
            version_id,
            primary_slot_id,
            target_copies,
            route_slot_ids,
            topology_generation,
            binding_generations,
        } = placement;
        Self {
            version_id,
            primary_slot_id,
            target_copies,
            route_slot_ids,
            topology_generation,
            binding_generations,
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/backend_storage_metadata.rs"]
mod tests;
