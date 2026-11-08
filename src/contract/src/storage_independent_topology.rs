//! Display-only graph from one verified independent disk. Deserialization
//! checks shape and references; it never creates disk or registration authority.
use crate::{
    storage_directories::DirectorySource,
    storage_independent_history::IndependentHistoryInventory,
    storage_independent_source::{
        IndependentCheckpoint, IndependentIdentity, IndependentRole, InvalidObservation,
        MAX_DISPLAY_PIN_REFERENCES, PublicationId, bounded, canonical_pins, pins,
    },
};
use data_serialization::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_TOPOLOGY_NODES: usize = 4096;
pub const MAX_TOPOLOGY_RELATIONS: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct IndependentDiskPreviewRequest {
    pub source: DirectorySource,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct WirePreview {
    source: DirectorySource,
    topology: IndependentSourceTopology,
    inventory: IndependentHistoryInventory,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(try_from = "WirePreview")]
pub struct IndependentDiskPreview {
    pub source: DirectorySource,
    pub topology: IndependentSourceTopology,
    pub inventory: IndependentHistoryInventory,
}
impl TryFrom<WirePreview> for IndependentDiskPreview {
    type Error = InvalidObservation;
    fn try_from(wire: WirePreview) -> Result<Self, Self::Error> {
        if wire.topology.root() != wire.inventory.root() {
            return Err(InvalidObservation::History);
        }
        // A retired owner can be absent from the current source graph. An
        // origin that is present must still name the same complete identity.
        {
            let known = wire
                .topology
                .nodes()
                .iter()
                .map(|node| (&node.identity.origin, &node.identity))
                .collect::<BTreeMap<_, _>>();
            for version in wire.inventory.versions() {
                if known
                    .get(&version.owner.origin)
                    .is_some_and(|identity| *identity != &version.owner)
                {
                    return Err(InvalidObservation::History);
                }
            }
        }
        Ok(Self {
            source: wire.source,
            topology: wire.topology,
            inventory: wire.inventory,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TopologyNode {
    pub identity: IndependentIdentity,
    #[serialization(deserialize_with = "pins")]
    pub pins: Vec<PublicationId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TopologyPoint {
    pub identity: IndependentIdentity,
    pub publication: PublicationId,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TopologyRelation {
    pub source: TopologyPoint,
    pub consumer: TopologyPoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serialization(rename_all = "snake_case")]
enum TopologyProtocol {
    IndependentSourceTopology,
}
impl<'de> Deserialize<'de> for TopologyProtocol {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match String::deserialize(d)?.as_str() {
            "independent_source_topology" => Ok(Self::IndependentSourceTopology),
            value => Err(data_serialization::de::Error::unknown_variant(
                value,
                &["independent_source_topology"],
            )),
        }
    }
}

fn nodes<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<TopologyNode>, D::Error> {
    bounded::<D, TopologyNode, MAX_TOPOLOGY_NODES>(d)
}
fn relations<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<TopologyRelation>, D::Error> {
    bounded::<D, TopologyRelation, MAX_TOPOLOGY_RELATIONS>(d)
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct WireTopology {
    protocol: TopologyProtocol,
    root: IndependentCheckpoint,
    #[serialization(deserialize_with = "nodes")]
    nodes: Vec<TopologyNode>,
    #[serialization(deserialize_with = "relations")]
    relations: Vec<TopologyRelation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(try_from = "WireTopology")]
pub struct IndependentSourceTopology {
    protocol: TopologyProtocol,
    root: IndependentCheckpoint,
    nodes: Vec<TopologyNode>,
    relations: Vec<TopologyRelation>,
}
impl IndependentSourceTopology {
    pub fn new(
        root: IndependentCheckpoint,
        nodes: Vec<TopologyNode>,
        relations: Vec<TopologyRelation>,
    ) -> Result<Self, InvalidObservation> {
        WireTopology {
            protocol: TopologyProtocol::IndependentSourceTopology,
            root,
            nodes,
            relations,
        }
        .try_into()
    }
    pub fn root(&self) -> &IndependentCheckpoint {
        &self.root
    }
    pub fn nodes(&self) -> &[TopologyNode] {
        &self.nodes
    }
    pub fn relations(&self) -> &[TopologyRelation] {
        &self.relations
    }
}

impl TryFrom<WireTopology> for IndependentSourceTopology {
    type Error = InvalidObservation;
    fn try_from(wire: WireTopology) -> Result<Self, Self::Error> {
        wire.root.validate()?;
        if wire.nodes.len() > MAX_TOPOLOGY_NODES || wire.relations.len() > MAX_TOPOLOGY_RELATIONS {
            return Err(InvalidObservation::TopologyBudget);
        }
        let mut origins = BTreeSet::new();
        let mut publications = BTreeMap::new();
        let mut root_found = false;
        for node in &wire.nodes {
            if !origins.insert(&node.identity.origin) || !canonical_pins(&node.pins) {
                return Err(InvalidObservation::Topology);
            }
            if node.identity == wire.root.identity {
                root_found = true;
                if node.pins.iter().ne(wire.root.publication.iter()) {
                    return Err(InvalidObservation::Topology);
                }
            } else if node.pins.is_empty()
                || node.identity.role.rank() >= wire.root.identity.role.rank()
            {
                return Err(InvalidObservation::Topology);
            }
            for pin in &node.pins {
                if publications.insert(pin, &node.identity).is_some() {
                    return Err(InvalidObservation::Topology);
                }
                if publications.len() > MAX_DISPLAY_PIN_REFERENCES {
                    return Err(InvalidObservation::PinBudget);
                }
            }
        }
        if !root_found {
            return Err(InvalidObservation::Topology);
        }
        let mut edges = BTreeSet::new();
        let mut sources = BTreeMap::<&PublicationId, Vec<&PublicationId>>::new();
        for relation in &wire.relations {
            let TopologyRelation { source, consumer } = relation;
            if source.identity.role.rank() + 1 != consumer.identity.role.rank()
                || source.identity.origin.disk_id == consumer.identity.origin.disk_id
                || publications.get(&source.publication) != Some(&&source.identity)
                || publications.get(&consumer.publication) != Some(&&consumer.identity)
                || !edges.insert((&source.publication, &consumer.publication))
            {
                return Err(InvalidObservation::Topology);
            }
            sources
                .entry(&consumer.publication)
                .or_default()
                .push(&source.publication);
        }
        // Every displayed publication must belong to this selected source
        // graph. Historical predecessors and unattached nodes are not edges.
        let mut visited = BTreeSet::new();
        let mut pending: Vec<_> = wire.root.publication.iter().collect();
        while let Some(pin) = pending.pop() {
            if !visited.insert(pin) {
                continue;
            }
            let identity = publications[pin];
            let upstream = sources.get(pin);
            if (identity.role == IndependentRole::Primary) != upstream.is_none() {
                return Err(InvalidObservation::Topology);
            }
            if let Some(upstream) = upstream {
                pending.extend(upstream.iter().copied());
            }
        }
        if visited.len() != publications.len() {
            return Err(InvalidObservation::Topology);
        }
        Ok(Self {
            protocol: wire.protocol,
            root: wire.root,
            nodes: wire.nodes,
            relations: wire.relations,
        })
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/storage_independent_topology.rs"]
mod tests;
