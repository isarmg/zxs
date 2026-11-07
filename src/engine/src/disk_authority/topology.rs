//! Metadata-only observations from the existing disk verifier, not a wire
//! format, current membership, content availability, or an authority capability.
use super::{model::Object, storage::LocalBinding, *};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Exact logical selection and held physical binding of one observation.
/// Equality compares selections; a separate capture can have an equal binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TopologyBinding {
    local: LocalBinding,
    identity: Identity,
    checkpoint: Option<Digest>,
    sequence: u64,
}
impl TopologyBinding {
    fn capture(verified: &VerifiedDisk, local: LocalBinding) -> Self {
        Self {
            local,
            identity: verified.identity.clone(),
            checkpoint: verified.checkpoint,
            sequence: verified.sequence,
        }
    }
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    pub fn checkpoint(&self) -> Option<Digest> {
        self.checkpoint
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}

#[derive(Clone, Debug)]
pub struct SourceNode {
    binding: Arc<TopologyBinding>,
    identity: Identity,
    checkpoints: BTreeSet<Digest>,
}
impl SourceNode {
    pub(super) fn same_node_capture(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.binding, &other.binding)
            && self.identity == other.identity
            && self.checkpoints == other.checkpoints
    }
    pub fn binding(&self) -> &TopologyBinding {
        &self.binding
    }
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    pub fn checkpoints(&self) -> &BTreeSet<Digest> {
        &self.checkpoints
    }
}

/// One exact publication, not whichever checkpoint is newest for its identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePoint {
    identity: Identity,
    publication: Digest,
}
impl SourcePoint {
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    pub fn publication(&self) -> Digest {
        self.publication
    }
}

/// Direction is source -> consumer. Both endpoints retain full identity and pin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceRelation {
    source: SourcePoint,
    consumer: SourcePoint,
}
impl SourceRelation {
    pub fn source(&self) -> &SourcePoint {
        &self.source
    }
    pub fn consumer(&self) -> &SourcePoint {
        &self.consumer
    }
}

#[derive(Clone, Debug)]
pub struct SourceTopology {
    binding: Arc<TopologyBinding>,
    nodes: BTreeMap<Origin, SourceNode>,
    relations: Vec<SourceRelation>,
}
impl SourceTopology {
    pub fn binding(&self) -> &TopologyBinding {
        &self.binding
    }
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = &SourceNode> {
        self.nodes.values()
    }
    pub fn relations(&self) -> &[SourceRelation] {
        &self.relations
    }
    /// A cloned node remains in this capture, but a later reread is a new
    /// capture even if its logical and physical selection is unchanged.
    pub fn contains_node(&self, node: &SourceNode) -> bool {
        Arc::ptr_eq(&self.binding, &node.binding)
    }
}

/// A result about one candidate only. Satisfying pins does not select a unique
/// physical disk or prove that a discovery scope was complete.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceCandidateStatus {
    IdentityMismatch,
    UnpinnedNode,
    MissingPinnedCheckpoints(BTreeSet<Digest>),
    SatisfiesPinnedCheckpoints,
}

#[derive(Clone, Debug)]
pub struct SourceCandidateObservation {
    node: SourceNode,
    candidate: TopologyBinding,
    status: SourceCandidateStatus,
}
impl SourceCandidateObservation {
    pub fn node(&self) -> &SourceNode {
        &self.node
    }
    pub fn candidate_binding(&self) -> &TopologyBinding {
        &self.candidate
    }
    pub fn status(&self) -> &SourceCandidateStatus {
        &self.status
    }
    /// Compare with the request-side node before using this historical result.
    /// A later topology capture or another node in this capture is not equal.
    pub fn is_for(&self, node: &SourceNode) -> bool {
        self.node.same_node_capture(node)
    }
}

pub(super) fn observe_candidate(
    node: &SourceNode,
    verified: &VerifiedDisk,
    local: LocalBinding,
) -> Result<SourceCandidateObservation> {
    let status = if verified.identity != node.identity {
        SourceCandidateStatus::IdentityMismatch
    } else if node.checkpoints.is_empty() {
        SourceCandidateStatus::UnpinnedNode
    } else {
        let mut missing = node.checkpoints.clone();
        let mut current = verified.checkpoint;
        // The original verifier already established this finite, contiguous
        // own chain. Source/history workset membership is not correspondence.
        while let Some(hash) = current {
            let publication = verified
                .publications
                .get(&hash)
                .ok_or(Error::Missing(hash))?;
            missing.remove(&hash);
            current = publication.previous;
        }
        if missing.is_empty() {
            SourceCandidateStatus::SatisfiesPinnedCheckpoints
        } else {
            SourceCandidateStatus::MissingPinnedCheckpoints(missing)
        }
    };
    Ok(SourceCandidateObservation {
        node: node.clone(),
        candidate: TopologyBinding::capture(verified, local),
        status,
    })
}

pub(super) fn project(verified: &VerifiedDisk, local: LocalBinding) -> Result<SourceTopology> {
    let binding = Arc::new(TopologyBinding::capture(verified, local));
    let mut nodes = BTreeMap::from([(
        verified.identity.origin.clone(),
        SourceNode {
            binding: Arc::clone(&binding),
            identity: verified.identity.clone(),
            checkpoints: BTreeSet::new(),
        },
    )]);
    let mut relations = BTreeMap::new();
    let mut pending: Vec<_> = verified.checkpoint.into_iter().collect();
    let mut visited = BTreeSet::new();
    while let Some(hash) = pending.pop() {
        if !visited.insert(hash) {
            continue;
        }
        let publication = verified
            .publications
            .get(&hash)
            .ok_or(Error::Missing(hash))?;
        let node = nodes
            .entry(publication.identity.origin.clone())
            .or_insert_with(|| SourceNode {
                binding: Arc::clone(&binding),
                identity: publication.identity.clone(),
                checkpoints: BTreeSet::new(),
            });
        node.checkpoints.insert(hash);
        let raw = verified
            .bundle
            .objects
            .get(&publication.manifest)
            .ok_or(Error::Missing(publication.manifest))?;
        let Object::Manifest(manifest) = codec::decode(raw)? else {
            return Err(Error::Invalid("verified publication has no manifest"));
        };
        let sources = match manifest.authority {
            Authority::Primary { .. } => continue,
            Authority::Secondary { sources } | Authority::Tertiary { sources } => sources,
        };
        for source in sources {
            relations.insert(
                (source.publication, hash),
                SourceRelation {
                    source: SourcePoint {
                        identity: source.identity,
                        publication: source.publication,
                    },
                    consumer: SourcePoint {
                        identity: publication.identity.clone(),
                        publication: hash,
                    },
                },
            );
            pending.push(source.publication);
        }
        // Previous is verified history, not the selected source graph.
    }
    Ok(SourceTopology {
        binding,
        nodes,
        relations: relations.into_values().collect(),
    })
}
