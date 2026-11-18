//! Bounded canonical order for local occurrence tables. The caller owns the
//! row type, ordering, provenance, and domain-specific row limit. A set of
//! pages alone cannot establish that any row exists or has been published.
use super::{ScopeClosureError as Failure, genesis::PAGE_LIMIT};
use crate::{
    DiskConfig, Result, disk::DiskRoot, journal,
    metadata_map::{Encoder, ProofMeasure, Reference},
};
use data_serialization::Serialize;

const PACKAGE_LIMIT: usize = 64 * 1024 * 1024;

pub(in crate::engine) struct OrdinalBuilder {
    rows: json_codec::Map<String, json_codec::Value>,
    limit: usize,
    input_bytes: usize,
}
impl OrdinalBuilder {
    pub(in crate::engine) fn new(limit: usize) -> Self {
        Self { rows: json_codec::Map::new(), limit, input_bytes: 0 }
    }

    /// The ordinal is local to this exact material. Matching object hashes
    /// across different occurrences must not remove/reorder rows.
    pub(in crate::engine) fn push<T: Serialize>(&mut self, row: &T) -> Result<()> {
        let key = format!("{:020}", self.rows.len());
        self.put(key, row)
    }
    fn put<T: Serialize>(&mut self, key: String, row: &T) -> Result<()> {
        if self.rows.contains_key(&key) { return Err(Failure::PageMismatch.into()); }
        if self.rows.len() >= self.limit { return Err(Failure::Budget.into()); }
        let value = json_codec::to_value(row)?;
        let bytes = storage_contract::canonical_json_value(&value)?.len();
        // Include escaped JSON map key, colon and the two braces. Typed keys
        // may themselves contain quotes; ordinal keys keep their old cost.
        let named = storage_contract::canonical_json(&key)?.len();
        let singleton = bytes.checked_add(named).and_then(|n| n.checked_add(3)).ok_or(Failure::Budget)?;
        if singleton > PAGE_LIMIT { return Err(Failure::Budget.into()); }
        self.input_bytes = self.input_bytes.checked_add(singleton - 1)
            .filter(|total| *total <= PACKAGE_LIMIT).ok_or(Failure::Budget)?;
        self.rows.insert(key, value);
        Ok(())
    }

    /// Consume only a sequential producer's original complete occurrences.
    /// Typed keyed native rows must not be reinterpreted as disk ordinals.
    pub(super) fn into_occurrences(mut self) -> Result<Vec<json_codec::Value>> {
        let n = self.rows.len();
        (0..n).map(|i| self.rows.remove(&format!("{i:020}"))
            .ok_or_else(|| Failure::PageMismatch.into())).collect()
    }

    pub(in crate::engine) fn finish(self) -> Result<OrdinalPages> {
        let entries = self.rows.len();
        let mut encoder = Encoder::new(PAGE_LIMIT);
        let root = encoder.map_root(self.rows, ProofMeasure::Plain)?.reference().clone();
        let pages = encoder.finish();
        let mut bytes = 0usize;
        for page in &pages {
            bytes = bytes.checked_add(page.bytes().len())
                .filter(|total| *total <= PACKAGE_LIMIT).ok_or(Failure::Budget)?;
        }
        Ok(OrdinalPages { root, pages, entries, bytes })
    }
}

/// Stable typed occurrence keys for an approved native material. This writer
/// is separate from ordinal rows: mixing the two representations is not valid.
pub(super) struct KeyedBuilder(OrdinalBuilder);
impl KeyedBuilder {
    pub(super) fn new(limit: usize) -> Self { Self(OrdinalBuilder::new(limit)) }
    pub(super) fn push<K: Serialize, V: Serialize>(&mut self, key: &K, value: &V) -> Result<()> {
        let key = String::from_utf8(storage_contract::canonical_json(key)?).map_err(|_| Failure::PageMismatch)?;
        self.0.put(key, value)
    }
    pub(super) fn finish(self) -> Result<OrdinalPages> { self.0.finish() }
}

pub(in crate::engine) struct OrdinalPages {
    root: Reference,
    pages: Vec<journal::PreparedMetadataObject>,
    entries: usize,
    bytes: usize,
}
impl OrdinalPages {
    pub(in crate::engine) fn root(&self) -> &Reference { &self.root }
    pub(in crate::engine) fn entries(&self) -> usize { self.entries }
    pub(in crate::engine) fn bytes(&self) -> usize { self.bytes }

    pub(in crate::engine) fn install(&self, disk: &DiskConfig, held: &DiskRoot) -> Result<()> {
        for page in &self.pages {
            page.store_at_root(held, disk)?;
        }
        self.verify(disk, held, &self.root)
    }

    pub(in crate::engine) fn verify(
        &self, disk: &DiskConfig, held: &DiskRoot, required: &Reference,
    ) -> Result<()> {
        if required != &self.root {
            return Err(Failure::PageMismatch.into());
        }
        // Only independently derived, bounded addresses. A hostile local
        // index cannot add reads, a suffix or substitute an equivalent map.
        for page in &self.pages {
            let actual = journal::load_scope_page_at(
                held, disk, page.kind(), &page.hash, PAGE_LIMIT,
            )?;
            if actual.bytes() != page.bytes() {
                return Err(Failure::PageMismatch.into());
            }
        }
        Ok(())
    }
}


#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/scope_ordinal_pages.rs"]
mod tests;
