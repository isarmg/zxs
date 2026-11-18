//! DISK change kind, source and ordering come from the original history
//! owner. Only bounded ordinal storage is shared with other holder materials.
use super::{MAX_CHANGES, StoredHistory, budget};
use crate::{
    DiskConfig, Result, disk::DiskRoot, engine::scope_closure::{OrdinalBuilder, OrdinalPages},
    metadata_map::Reference,
};

pub(super) struct ChangePages(OrdinalPages);
impl ChangePages {
    pub(super) fn prepare(history: &StoredHistory) -> Result<Self> {
        if history.changes.len() > MAX_CHANGES {
            return Err(budget());
        }
        let mut entries = OrdinalBuilder::new(MAX_CHANGES);
        for row in &history.changes {
            entries.push(row)?;
        }
        Ok(Self(entries.finish()?))
    }
    pub(super) fn root(&self) -> &Reference { self.0.root() }
    pub(super) fn bytes(&self) -> usize { self.0.bytes() }
    pub(super) fn install(&self, disk: &DiskConfig, root: &DiskRoot) -> Result<()> {
        self.0.install(disk, root)
    }
    pub(super) fn verify(
        &self, disk: &DiskConfig, root: &DiskRoot, required: &Reference,
    ) -> Result<()> {
        self.0.verify(disk, root, required)
    }
}
