//! An immutable historical choice is independent of the current workspace CAS.
use super::DirectoryBranchCreateSource;
use crate::DirectoryTreeId;
use data_serialization::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serialization(try_from = "SourceWire")]
pub struct HistoricalDirectoryBranchSource {
    current: DirectoryBranchCreateSource,
    tree_id: DirectoryTreeId,
}

impl HistoricalDirectoryBranchSource {
    pub fn new(
        current: DirectoryBranchCreateSource,
        tree_id: DirectoryTreeId,
    ) -> crate::Result<Self> {
        current.validate()?;
        Ok(Self { current, tree_id })
    }

    pub fn current(&self) -> &DirectoryBranchCreateSource {
        &self.current
    }

    pub fn tree_id(&self) -> DirectoryTreeId {
        self.tree_id
    }
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct SourceWire {
    current: DirectoryBranchCreateSource,
    tree_id: DirectoryTreeId,
}

impl TryFrom<SourceWire> for HistoricalDirectoryBranchSource {
    type Error = crate::Error;

    fn try_from(wire: SourceWire) -> Result<Self, Self::Error> {
        Self::new(wire.current, wire.tree_id)
    }
}

#[cfg(test)]
#[path = "../../../../test/contract/unit/historical_directory_branch.rs"]
mod tests;
