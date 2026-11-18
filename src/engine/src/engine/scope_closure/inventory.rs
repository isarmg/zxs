//! Cold inclusion must start at an independently selected native Snapshot. A
//! real local object with the same logical rows cannot select its own bytes.
use super::{HistoryReference, OrdinalBuilder, ScopeClosureError as Failure, genesis::PAGE_LIMIT};
use crate::{DiskConfig, Result, disk::DiskRoot, journal, metadata_map};
use std::collections::BTreeMap;
use storage_contract::working_tree::DiskRegistrationIdentity;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Section { Namespace, File, Backup }
fn classify(row: &json_codec::Value) -> Result<Section> {
    let fields = row.as_object().ok_or(Failure::PageMismatch)?;
    const REQUIRED: [&str; 6] = ["object","ownership","location","lifecycle","entity","operation_id"];
    if fields.len() != REQUIRED.len() || REQUIRED.iter().any(|k| !fields.contains_key(*k)) {
        return Err(Failure::PageMismatch.into());
    }
    match fields["object"].get("kind").and_then(json_codec::Value::as_str) {
        Some("ordinary_directory" | "directory_repository" | "directory_repository_reservation" | "worktree") => Ok(Section::Namespace),
        Some("file_container") => Ok(Section::File),
        Some("backup_replica") => Ok(Section::Backup),
        _ => Err(Failure::PageMismatch.into()),
    }
}
struct Subset {
    selected: Section, expected: BTreeMap<Vec<u8>, usize>, ordered: BTreeMap<usize, usize>, all: Vec<bool>,
}
impl Subset {
    fn new(selected: Section, n: u64, rows: OrdinalBuilder) -> Result<Self> {
        let n = usize::try_from(n).ok().filter(|n| *n <= 65_536).ok_or(Failure::Budget)?;
        let mut expected = BTreeMap::new();
        for (i, row) in rows.into_occurrences()?.into_iter().enumerate() {
            if classify(&row)? != selected || expected.insert(storage_contract::canonical_json_value(&row)?, i).is_some() {
                return Err(Failure::PageMismatch.into());
            }
        }
        if expected.len() > n { return Err(Failure::PageMismatch.into()); }
        Ok(Self { selected, expected, ordered: BTreeMap::new(), all: vec![false; n] })
    }
    fn leaf(&mut self, page: json_codec::Value) -> Result<()> {
        let json_codec::Value::Object(page) = page else { return Err(Failure::PageMismatch.into()); };
        if page.len() > 128 { return Err(Failure::Budget.into()); }
        for (key,row) in page {
            let i = key.parse::<usize>().map_err(|_| Failure::PageMismatch)?;
            if key != format!("{i:020}") || i >= self.all.len() || std::mem::replace(&mut self.all[i], true) {
                return Err(Failure::PageMismatch.into());
            }
            if classify(&row)? == self.selected {
                let occurrence = self.expected.remove(&storage_contract::canonical_json_value(&row)?).ok_or(Failure::PageMismatch)?;
                self.ordered.insert(i, occurrence);
            }
        }
        Ok(())
    }
    fn finish(self) -> Result<usize> {
        let n = self.ordered.len();
        if self.all.iter().any(|seen| !*seen) || !self.expected.is_empty()
            || !self.ordered.into_values().enumerate().all(|(n, original)| n == original) {
            return Err(Failure::PageMismatch.into());
        }
        Ok(n)
    }
}
impl HistoryReference {
    /// The containing original Decision has authenticated the address and
    /// registration. Check the selected holder and complete ordinal coverage, then
    /// match this one owner's full independently generated rows byte-for-byte.
    /// Nothing asserts availability of the ignored owners or other disks.
    pub(super) fn require_section(&self, disk: &DiskConfig, root: &DiskRoot,
        registration: &DiskRegistrationIdentity, sequence: u64, section: Section, rows: OrdinalBuilder) -> Result<usize> {
        if self.selected_registration != *registration || disk.disk_id != registration.disk_id
            || disk.slot_id != registration.slot_id || self.selected_sequence.get() != sequence {
            return Err(Failure::PublicationMismatch.into());
        }
        let mut subset = Subset::new(section, self.inventory_entries, rows)?;
        let mut bytes = 0usize;
        metadata_map::visit_objects(&self.inventory, |kind, hash| {
            let object = journal::load_scope_page_at(root, disk, kind, hash, PAGE_LIMIT)?;
            bytes = bytes.checked_add(object.bytes().len()).filter(|n| *n <= 64 * 1024 * 1024).ok_or(Failure::Budget)?;
            Ok(object)
        }, |object| {
            if object.kind() == "state-page" { subset.leaf(object.into_value())?; }
            Ok(())
        })?;
        subset.finish()
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/scope_inventory_rows.rs"]
mod tests;
