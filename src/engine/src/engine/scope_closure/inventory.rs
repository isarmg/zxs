//! Cold inclusion must start at an independently selected native Snapshot. A
//! real local object with the same logical rows cannot select its own bytes.
pub(super) mod row;
use super::{HistoryReference, OrdinalBuilder, ScopeClosureError as Failure, genesis::PAGE_LIMIT};
use crate::{DiskConfig, Result, disk::DiskRoot, journal, metadata_map};
use std::collections::BTreeMap;
use storage_contract::working_tree::DiskRegistrationIdentity;

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
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
    // Order is native to each owner; global leaf traversal and the order
    // of unrelated owners cannot determine a selected FILE's predecessor.
    selected: BTreeMap<Section,usize>, expected: BTreeMap<Vec<u8>, (Section,usize)>,
    ordered: BTreeMap<usize, (Section,usize)>, all: Vec<bool>,
}
impl Subset {
    fn new(selected: Section, n: u64, rows: OrdinalBuilder) -> Result<Self> {
        let n = usize::try_from(n).ok().filter(|n| *n <= 65_536).ok_or(Failure::Budget)?;
        let mut value = Self { selected: BTreeMap::new(), expected: BTreeMap::new(),
            ordered: BTreeMap::new(), all: vec![false; n] };
        value.include(selected,rows)?;
        Ok(value)
    }
    fn include(&mut self, section: Section, rows: OrdinalBuilder) -> Result<()> {
        if self.selected.insert(section,0).is_some() { return Err(Failure::PageMismatch.into()); }
        for (i, row) in rows.into_occurrences()?.into_iter().enumerate() {
            if classify(&row)? != section
                || self.expected.insert(storage_contract::canonical_json_value(&row)?, (section,i)).is_some() {
                return Err(Failure::PageMismatch.into());
            }
        }
        if self.expected.len() > self.all.len() { return Err(Failure::PageMismatch.into()); }
        Ok(())
    }
    fn leaf(&mut self, page: json_codec::Value) -> Result<()> {
        let json_codec::Value::Object(page) = page else { return Err(Failure::PageMismatch.into()); };
        if page.len() > 128 { return Err(Failure::Budget.into()); }
        for (key,row) in page {
            let i = key.parse::<usize>().map_err(|_| Failure::PageMismatch)?;
            if key != format!("{i:020}") || i >= self.all.len() || std::mem::replace(&mut self.all[i], true) {
                return Err(Failure::PageMismatch.into());
            }
            if self.selected.contains_key(&classify(&row)?) {
                let occurrence = self.expected.remove(&storage_contract::canonical_json_value(&row)?).ok_or(Failure::PageMismatch)?;
                self.ordered.insert(i, occurrence);
            }
        }
        Ok(())
    }
    fn finish(mut self) -> Result<usize> {
        let n = self.ordered.len();
        if self.all.iter().any(|seen| !*seen) || !self.expected.is_empty() { return Err(Failure::PageMismatch.into()); }
        for (section, original) in self.ordered.into_values() {
            let rank = self.selected.get_mut(&section).ok_or(Failure::PageMismatch)?;
            if *rank != original { return Err(Failure::PageMismatch.into()); }
            *rank += 1;
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
        self.read_inventory(disk,root,registration,sequence,Subset::new(section, self.inventory_entries, rows)?)
    }
    pub(super) fn require_business_sections(&self, disk: &DiskConfig, root: &DiskRoot,
        registration: &DiskRegistrationIdentity, sequence: u64, namespaces: OrdinalBuilder, files: OrdinalBuilder) -> Result<usize> {
        let mut selected = Subset::new(Section::Namespace, self.inventory_entries, namespaces)?;
        selected.include(Section::File,files)?;
        self.read_inventory(disk,root,registration,sequence,selected)
    }
    fn read_inventory(&self, disk: &DiskConfig, root: &DiskRoot,
        registration: &DiskRegistrationIdentity, sequence: u64, mut subset: Subset) -> Result<usize> {
        if self.selected_registration != *registration || disk.disk_id != registration.disk_id
            || disk.slot_id != registration.slot_id || self.selected_sequence.get() != sequence {
            return Err(Failure::PublicationMismatch.into());
        }
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
