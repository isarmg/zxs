//! Bounded observations of explicitly selected, held candidate directories.
//! No producer here can assert complete discovery or choose a unique disk.
use super::{
    Error, SourceCandidateObservation, SourceCandidateStatus, SourceNode, location::Location,
    storage::CapturedSourceCandidate,
};
use std::collections::BTreeMap;

pub(super) const MAX_CANDIDATES: usize = 128;
pub(super) const MAX_CHAIN_ENTRIES: u64 = 10_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceCandidateBatchStage {
    OpenDirectory,
    Capture,
    RecheckDirectory,
}

#[derive(Debug, error_definition::Error)]
pub enum SourceCandidateBatchError {
    #[error("candidate count {actual} exceeds {maximum}")]
    CandidateBudget { actual: usize, maximum: usize },
    #[error(
        "candidate own-chain entries exceed {maximum}; aliases are charged before deduplication"
    )]
    CheckpointBudget { maximum: u64 },
    #[error("candidate {index} failed during {stage:?}: {source}")]
    Candidate {
        index: usize,
        stage: SourceCandidateBatchStage,
        #[source]
        source: Error,
    },
    #[error("candidate {index} was observed for another source-node capture")]
    DifferentSourceCapture { index: usize },
    #[error("cannot establish the physical relation of candidates {first} and {second}: {source}")]
    PhysicalRelation {
        first: usize,
        second: usize,
        #[source]
        source: Error,
    },
    #[error("candidate directories {first} and {second} overlap")]
    OverlappingCandidates { first: usize, second: usize },
    #[error("the same physical directory changed between candidates {first} and {second}")]
    ConflictingPhysicalObservation { first: usize, second: usize },
}

/// Ambiguity can be established from two matches even in a partial observation.
/// Fewer matches never imply global absence or uniqueness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceCandidateBatchStatus {
    NotObserved,
    IncompleteObservation { matching_candidates: usize },
    Ambiguous { matching_candidates: usize },
}

#[derive(Clone, Debug)]
pub struct SourceCandidateBatch {
    node: SourceNode,
    observations: Vec<SourceCandidateObservation>,
    physical_candidates: usize,
    status: SourceCandidateBatchStatus,
}
impl SourceCandidateBatch {
    pub fn node(&self) -> &SourceNode {
        &self.node
    }
    /// Preserve every requested observation, including aliases and rejections.
    /// Input order is descriptive and never chooses a preferred matching disk.
    pub fn observations(&self) -> &[SourceCandidateObservation] {
        &self.observations
    }
    pub fn physical_candidates(&self) -> usize {
        self.physical_candidates
    }
    pub fn status(&self) -> &SourceCandidateBatchStatus {
        &self.status
    }
    pub fn is_for(&self, node: &SourceNode) -> bool {
        self.node.same_node_capture(node)
    }
}

struct CapturedRoot {
    first_selection: usize,
    observation_index: usize,
    location: Location,
}

pub(super) struct CandidateBatchBuilder {
    node: SourceNode,
    observations: Vec<SourceCandidateObservation>,
    roots: Vec<CapturedRoot>,
    by_physical: BTreeMap<(u64, u64), usize>,
    chain_entries: u64,
    matching: usize,
}
impl CandidateBatchBuilder {
    pub(super) fn new(node: &SourceNode, count: usize) -> Result<Self, SourceCandidateBatchError> {
        if count > MAX_CANDIDATES {
            return Err(SourceCandidateBatchError::CandidateBudget {
                actual: count,
                maximum: MAX_CANDIDATES,
            });
        }
        Ok(Self {
            node: node.clone(),
            observations: Vec::with_capacity(count),
            roots: Vec::new(),
            by_physical: BTreeMap::new(),
            chain_entries: 0,
            matching: 0,
        })
    }
    pub(super) fn push(
        &mut self,
        index: usize,
        capture: CapturedSourceCandidate,
    ) -> Result<(), SourceCandidateBatchError> {
        let (location, observation) = capture.into_parts();
        if !observation.is_for(&self.node) {
            return Err(SourceCandidateBatchError::DifferentSourceCapture { index });
        }
        let charged = charge_chain_entries(
            self.chain_entries,
            observation.candidate_binding().sequence(),
        )?;
        let identity = location.identity();
        let key = (identity.device, identity.inode);
        if let Some(&previous) = self.by_physical.get(&key) {
            let previous = &self.roots[previous];
            let overlap = relation(previous, &location, index)?;
            debug_assert!(
                overlap,
                "invariant: identical held directory identities overlap"
            );
            if !same_facts(&self.observations[previous.observation_index], &observation) {
                return Err(SourceCandidateBatchError::ConflictingPhysicalObservation {
                    first: previous.first_selection,
                    second: index,
                });
            }
        } else {
            for previous in &self.roots {
                if relation(previous, &location, index)? {
                    return Err(SourceCandidateBatchError::OverlappingCandidates {
                        first: previous.first_selection,
                        second: index,
                    });
                }
            }
            self.by_physical.insert(key, self.roots.len());
            self.roots.push(CapturedRoot {
                first_selection: index,
                observation_index: self.observations.len(),
                location,
            });
            if matches!(
                observation.status(),
                SourceCandidateStatus::SatisfiesPinnedCheckpoints
            ) {
                self.matching += 1;
            }
        }
        self.observations.push(observation);
        self.chain_entries = charged;
        Ok(())
    }
    pub(super) fn finish(self) -> SourceCandidateBatch {
        let status = if self.observations.is_empty() {
            SourceCandidateBatchStatus::NotObserved
        } else if self.matching < 2 {
            SourceCandidateBatchStatus::IncompleteObservation {
                matching_candidates: self.matching,
            }
        } else {
            SourceCandidateBatchStatus::Ambiguous {
                matching_candidates: self.matching,
            }
        };
        SourceCandidateBatch {
            node: self.node,
            observations: self.observations,
            physical_candidates: self.roots.len(),
            status,
        }
    }
}

fn relation(
    previous: &CapturedRoot,
    location: &Location,
    second: usize,
) -> Result<bool, SourceCandidateBatchError> {
    previous.location.overlaps(location).map_err(|source| {
        SourceCandidateBatchError::PhysicalRelation {
            first: previous.first_selection,
            second,
            source,
        }
    })
}

fn same_facts(first: &SourceCandidateObservation, second: &SourceCandidateObservation) -> bool {
    let first_binding = first.candidate_binding();
    let second_binding = second.candidate_binding();
    first_binding.identity() == second_binding.identity()
        && first_binding.checkpoint() == second_binding.checkpoint()
        && first_binding.sequence() == second_binding.sequence()
        && first.status() == second.status()
}

pub(super) fn charge_chain_entries(
    total: u64,
    next: u64,
) -> Result<u64, SourceCandidateBatchError> {
    total
        .checked_add(next)
        .filter(|sum| *sum <= MAX_CHAIN_ENTRIES)
        .ok_or(SourceCandidateBatchError::CheckpointBudget {
            maximum: MAX_CHAIN_ENTRIES,
        })
}
