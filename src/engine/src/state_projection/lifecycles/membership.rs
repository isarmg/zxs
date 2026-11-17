//! Bounded descriptive receipt membership, not a decoded audit or read grant.
use super::{DeletionOperations, DeletionOwner};
use crate::Result;
use std::collections::BTreeMap;
use storage_contract::{OperationId, history_window::HistoryFileId};

#[derive(Clone, Default)]
pub(super) struct FileMembership {
    pub(super) deletions: DeletionOperations,
    pub(super) copies: DeletionOperations,
}

/// At most one fixed-size file/operation membership per counted item. No State,
/// raw receipt, principal, historical node body or decoded copy is retained.
#[derive(Default)]
pub(crate) struct LifecycleMembershipIndex {
    files: BTreeMap<HistoryFileId, FileMembership>,
    count: usize,
}

impl LifecycleMembershipIndex {
    pub(super) fn observe(
        &mut self,
        operation: OperationId,
        owner: &DeletionOwner<'_>,
        copy: bool,
    ) -> Result<bool> {
        match owner {
            DeletionOwner::Other | DeletionOwner::Malformed => Ok(true),
            DeletionOwner::File(file) => self.insert(operation, file, copy),
            DeletionOwner::Files(files) => {
                for file in files {
                    if !self.insert(operation, file, copy)? { return Ok(false); }
                }
                Ok(true)
            }
        }
    }

    fn insert(&mut self, operation: OperationId, file: &str, copy: bool) -> Result<bool> {
        if self.count == storage_contract::file_lifecycle::MAX_AUDIT_EVENTS {
            return Ok(false);
        }
        let file = HistoryFileId::parse(file)?;
        let row = self.files.entry(file).or_default();
        let members = if copy { &mut row.copies } else { &mut row.deletions };
        if members.insert(operation, ()).is_none() { self.count += 1; }
        Ok(true)
    }

    pub(super) fn for_file(&self, file: &HistoryFileId) -> FileMembership {
        self.files.get(file).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/state_projection/lifecycle_membership.rs"]
mod tests;
