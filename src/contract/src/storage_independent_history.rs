//! Read-only display of versions retained in one verified disk publication.
//! These are metadata references, never content availability or write authority.
use crate::storage_independent_source::{
    IndependentBodyId, IndependentCheckpoint, IndependentCommitId, IndependentIdentity,
    IndependentRole, IndependentToken, InvalidObservation, bounded,
};
use data_serialization::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_RETAINED_VERSIONS: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct RetainedVersion {
    pub owner: IndependentIdentity,
    pub history_id: IndependentToken,
    pub file_id: IndependentToken,
    pub commit: IndependentCommitId,
    pub body: IndependentBodyId,
}

fn versions<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<RetainedVersion>, D::Error> {
    bounded::<D, RetainedVersion, MAX_RETAINED_VERSIONS>(d)
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct WireInventory {
    root: IndependentCheckpoint,
    #[serialization(deserialize_with = "versions")]
    versions: Vec<RetainedVersion>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(try_from = "WireInventory")]
pub struct IndependentHistoryInventory {
    root: IndependentCheckpoint,
    versions: Vec<RetainedVersion>,
}
impl IndependentHistoryInventory {
    pub fn new(
        root: IndependentCheckpoint,
        versions: Vec<RetainedVersion>,
    ) -> Result<Self, InvalidObservation> {
        WireInventory { root, versions }.try_into()
    }
    pub fn root(&self) -> &IndependentCheckpoint {
        &self.root
    }
    pub fn versions(&self) -> &[RetainedVersion] {
        &self.versions
    }
}
impl TryFrom<WireInventory> for IndependentHistoryInventory {
    type Error = InvalidObservation;
    fn try_from(wire: WireInventory) -> Result<Self, Self::Error> {
        wire.root.validate()?;
        if wire.versions.len() > MAX_RETAINED_VERSIONS {
            return Err(InvalidObservation::HistoryBudget);
        }
        if wire.root.publication.is_none() && !wire.versions.is_empty() {
            return Err(InvalidObservation::History);
        }
        let mut commits = BTreeSet::new();
        let mut owners = BTreeMap::new();
        for version in &wire.versions {
            if version.owner.role != IndependentRole::Primary
                || (wire.root.identity.role == IndependentRole::Primary
                    && version.owner != wire.root.identity)
                || !commits.insert(&version.commit)
                || owners
                    .insert(&version.owner.origin, &version.owner)
                    .is_some_and(|previous| previous != &version.owner)
            {
                return Err(InvalidObservation::History);
            }
        }
        Ok(Self {
            root: wire.root,
            versions: wire.versions,
        })
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/storage_independent_history.rs"]
mod tests;
