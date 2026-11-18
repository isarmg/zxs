//! A local manifest points to exact, ordered original change rows. Canonical
//! SQLite and this radix map share their source, not a database byte encoder.
use super::{MAX_CHANGES, StoredHistory, budget, invalid};
use crate::{
    DiskConfig, Result, disk::DiskRoot, journal,
    metadata_map::{Encoder, ProofMeasure, Reference},
};

const PAGE_LIMIT: usize = 64 * 1024;
pub(super) struct ChangePages {
    root: Reference,
    pages: Vec<journal::PreparedMetadataObject>,
    bytes: usize,
}
impl ChangePages {
    pub(super) fn prepare(history: &StoredHistory) -> Result<Self> {
        if history.changes.len() > MAX_CHANGES {
            return Err(budget());
        }
        let mut rows = json_codec::Map::new();
        for (ordinal, change) in history.changes.iter().enumerate() {
            rows.insert(format!("{ordinal:020}"), json_codec::to_value(change)?);
        }
        let mut encoder = Encoder::new(PAGE_LIMIT);
        let root = encoder
            .map_root(rows, ProofMeasure::Plain)?
            .reference()
            .clone();
        let pages = encoder.finish();
        let bytes = pages.iter().try_fold(0usize, |used, page| {
            used.checked_add(page.bytes().len())
                .filter(|n| *n <= 64 * 1024 * 1024)
                .ok_or_else(budget)
        })?;
        Ok(Self { root, pages, bytes })
    }

    pub(super) fn root(&self) -> &Reference {
        &self.root
    }

    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }

    pub(super) fn install(&self, disk: &DiskConfig, root: &DiskRoot) -> Result<()> {
        for page in &self.pages {
            page.store_at_root(root, disk)?;
        }
        self.verify(disk, root, &self.root)
    }

    pub(super) fn verify(
        &self,
        disk: &DiskConfig,
        root: &DiskRoot,
        required: &Reference,
    ) -> Result<()> {
        if required != &self.root {
            return Err(invalid("manifest does not bind every original DISK change"));
        }
        // Use the finite independently derived page set. A hostile local index
        // cannot cause unbounded recursive reads or omit a later ordinal.
        for page in &self.pages {
            let actual = journal::load_scope_page_at(
                root, disk, page.kind(), &page.hash, PAGE_LIMIT,
            )?;
            if actual.bytes() != page.bytes() {
                return Err(invalid("original DISK change page differs"));
            }
        }
        Ok(())
    }
}
