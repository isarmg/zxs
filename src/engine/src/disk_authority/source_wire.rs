//! One-way transport adapter from real verifier output. Decoded display values
//! cannot construct a batch, a request completion, or any storage authority.
use super::{
    Digest, Identity, Origin, Role, SourceCandidateBatch, SourceCandidateBatchStatus,
    SourceCandidateStatus, SourceNode, TopologyBinding,
};
use std::io::{self, Write};
use storage_contract::storage_independent_source::{
    BatchStatus, BindingGeneration, CandidateRow, CandidateStatus, Incarnation,
    IndependentCheckpoint, IndependentIdentity, IndependentNodeReference, IndependentOrigin,
    IndependentRole, IndependentSourceObservation, IndependentToken, InvalidObservation,
    MAX_DISPLAY_PIN_REFERENCES, MAX_NODE_PINS, MAX_OBSERVATION_BYTES, PublicationId,
};
use storage_contract::storage_independent_topology::{
    IndependentSourceTopology, MAX_TOPOLOGY_NODES, MAX_TOPOLOGY_RELATIONS, TopologyNode,
    TopologyPoint, TopologyRelation,
};

/// Display retained metadata only. In particular, absence of body reads must
/// never be converted into Available or TemporarilyUnavailable observations.
pub fn source_inventory_display(
    inventory: &super::SourceInventory,
) -> Result<
    storage_contract::storage_independent_history::IndependentHistoryInventory,
    InvalidObservation,
> {
    use storage_contract::{
        storage_independent_history::{
            IndependentHistoryInventory, MAX_RETAINED_VERSIONS, RetainedVersion,
        },
        storage_independent_source::{IndependentBodyId, IndependentCommitId},
    };
    if inventory.versions().len() > MAX_RETAINED_VERSIONS {
        return Err(InvalidObservation::HistoryBudget);
    }
    let versions = inventory
        .versions()
        .iter()
        .map(|version| {
            Ok(RetainedVersion {
                owner: identity(version.owner())?,
                history_id: IndependentToken::parse(version.history_id().as_str())?,
                file_id: IndependentToken::parse(version.file_id().as_str())?,
                commit: IndependentCommitId::parse(&version.commit().to_string())?,
                body: IndependentBodyId::parse(&version.body().to_string())?,
            })
        })
        .collect::<Result<_, InvalidObservation>>()?;
    IndependentHistoryInventory::new(checkpoint(inventory.topology().binding())?, versions)
}

/// Project every exact source/publication edge from the real held-disk capture.
/// This is historical display data, not current presence or an import permit.
pub fn source_topology_display(
    topology: &super::SourceTopology,
) -> Result<IndependentSourceTopology, InvalidObservation> {
    if topology.nodes().len() > MAX_TOPOLOGY_NODES
        || topology.relations().len() > MAX_TOPOLOGY_RELATIONS
    {
        return Err(InvalidObservation::TopologyBudget);
    }
    let mut references = 0;
    let nodes = topology
        .nodes()
        .map(|node| {
            if node.checkpoints().len() > MAX_NODE_PINS
                || node.checkpoints().len() > MAX_DISPLAY_PIN_REFERENCES - references
            {
                return Err(InvalidObservation::PinBudget);
            }
            references += node.checkpoints().len();
            Ok(TopologyNode {
                identity: identity(node.identity())?,
                pins: node
                    .checkpoints()
                    .iter()
                    .copied()
                    .map(publication)
                    .collect::<Result<_, _>>()?,
            })
        })
        .collect::<Result<_, _>>()?;
    let relations = topology
        .relations()
        .iter()
        .map(|relation| {
            let point = |point: &super::SourcePoint| -> Result<TopologyPoint, InvalidObservation> {
                Ok(TopologyPoint {
                    identity: identity(point.identity())?,
                    publication: publication(point.publication())?,
                })
            };
            Ok(TopologyRelation {
                source: point(relation.source())?,
                consumer: point(relation.consumer())?,
            })
        })
        .collect::<Result<_, InvalidObservation>>()?;
    IndependentSourceTopology::new(checkpoint(topology.binding())?, nodes, relations)
}

#[derive(Debug, error_definition::Error)]
pub enum SourceObservationEncodingError {
    #[error("verified source observation exceeds or differs from the display contract: {0}")]
    Contract(#[from] InvalidObservation),
    #[error("source observation JSON encoding failed: {0}")]
    Json(#[from] json_codec::Error),
}

/// Encode one real batch as a bounded successful response body. Callers must
/// first admit the request's response into its owning SourceCandidateSession.
/// This adapter supplies no HTTP authentication, scope, request ID, complete
/// graph transport, discovery completeness, or ongoing liveness guarantee.
pub fn encode_source_observation(
    batch: &SourceCandidateBatch,
) -> Result<Vec<u8>, SourceObservationEncodingError> {
    let observation = project(batch)?;
    let mut output = BoundedOutput(Vec::new());
    json_codec::to_writer(&mut output, &observation)?;
    Ok(output.0)
}

fn project(
    batch: &SourceCandidateBatch,
) -> Result<IndependentSourceObservation, InvalidObservation> {
    let mut references = batch.node().checkpoints().len();
    if references > MAX_NODE_PINS {
        return Err(InvalidObservation::Node);
    }
    let node = node_reference(batch.node())?;
    let observations = batch
        .observations()
        .iter()
        .map(|row| {
            Ok(CandidateRow {
                checkpoint: checkpoint(row.candidate_binding())?,
                status: match row.status() {
                    SourceCandidateStatus::IdentityMismatch => CandidateStatus::IdentityMismatch,
                    SourceCandidateStatus::UnpinnedNode => CandidateStatus::UnpinnedNode,
                    SourceCandidateStatus::MissingPinnedCheckpoints(missing) => {
                        if missing.len() > MAX_DISPLAY_PIN_REFERENCES - references {
                            return Err(InvalidObservation::PinBudget);
                        }
                        references += missing.len();
                        CandidateStatus::MissingPinnedCheckpoints {
                            missing: missing
                                .iter()
                                .copied()
                                .map(publication)
                                .collect::<Result<_, _>>()?,
                        }
                    }
                    SourceCandidateStatus::SatisfiesPinnedCheckpoints => {
                        CandidateStatus::SatisfiesPinnedCheckpoints
                    }
                },
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let status = match *batch.status() {
        SourceCandidateBatchStatus::NotObserved => BatchStatus::NotObserved,
        SourceCandidateBatchStatus::IncompleteObservation {
            matching_candidates,
        } => BatchStatus::IncompleteObservation {
            matching_candidates,
        },
        SourceCandidateBatchStatus::Ambiguous {
            matching_candidates,
        } => BatchStatus::Ambiguous {
            matching_candidates,
        },
    };
    IndependentSourceObservation::new(node, observations, batch.physical_candidates(), status)
}

fn node_reference(node: &SourceNode) -> Result<IndependentNodeReference, InvalidObservation> {
    Ok(IndependentNodeReference {
        root: checkpoint(node.binding())?,
        identity: identity(node.identity())?,
        pins: node
            .checkpoints()
            .iter()
            .copied()
            .map(publication)
            .collect::<Result<_, _>>()?,
    })
}
fn checkpoint(binding: &TopologyBinding) -> Result<IndependentCheckpoint, InvalidObservation> {
    Ok(IndependentCheckpoint {
        identity: identity(binding.identity())?,
        publication: binding.checkpoint().map(publication).transpose()?,
        sequence: binding.sequence(),
    })
}
fn publication(digest: Digest) -> Result<PublicationId, InvalidObservation> {
    PublicationId::parse(&digest.to_string())
}
fn identity(identity: &Identity) -> Result<IndependentIdentity, InvalidObservation> {
    let Identity {
        origin,
        incarnation,
        role,
    } = identity;
    let Origin {
        pool_id,
        disk_id,
        slot_id,
        binding_generation,
    } = origin;
    Ok(IndependentIdentity {
        origin: IndependentOrigin {
            pool_id: IndependentToken::parse(pool_id.as_str())?,
            disk_id: IndependentToken::parse(disk_id.as_str())?,
            slot_id: IndependentToken::parse(slot_id.as_str())?,
            binding_generation: BindingGeneration::new(*binding_generation)?,
        },
        incarnation: Incarnation::parse(&incarnation.to_string())?,
        role: match role {
            Role::Primary => IndependentRole::Primary,
            Role::Secondary => IndependentRole::Secondary,
            Role::Tertiary => IndependentRole::Tertiary,
        },
    })
}

struct BoundedOutput(Vec<u8>);
impl Write for BoundedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_OBSERVATION_BYTES - self.0.len() {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "independent observation exceeds response byte budget",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/disk_authority/source_wire.rs"]
mod tests;
