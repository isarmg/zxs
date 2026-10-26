//! A transaction-owned namespace index, not a published-tree or read cache.
//!
//! Slots retain the exact order of the former `retain` followed by `push`:
//! replacing an entry leaves a hole and appends its replacement. Flatten once
//! before ordinary namespace operations, whose deletion ordering is observable.
use super::*;

pub(crate) struct NamespaceDraft {
    slots: Vec<Option<Entry>>,
    by_path: BTreeMap<String, usize>,
    #[cfg(test)]
    work: std::rc::Rc<std::cell::Cell<Work>>,
}

#[cfg(test)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Work {
    source_visits: usize,
    map_operations: usize,
    appended_slots: usize,
    flattened_slots: usize,
}

impl NamespaceDraft {
    /// Start from admitted commit entries; no path normalization or admission work.
    pub(crate) fn new(entries: Vec<Entry>) -> Self {
        let mut draft = Self {
            slots: Vec::with_capacity(entries.len()),
            by_path: BTreeMap::new(),
            #[cfg(test)]
            work: Default::default(),
        };
        for entry in entries {
            #[cfg(test)]
            draft.record(|work| work.source_visits += 1);
            assert!(
                draft
                    .index_insert(entry.lookup_key().to_owned(), draft.slots.len())
                    .is_none(),
                "invariant: admitted namespace lookup keys are unique"
            );
            draft.slots.push(Some(entry));
        }
        draft
    }

    pub(crate) fn get_key(&self, key: &str) -> Option<&Entry> {
        #[cfg(test)]
        self.record(|work| work.map_operations += 1);
        self.by_path
            .get(key)
            .and_then(|&slot| self.slots[slot].as_ref())
    }

    fn index_insert(&mut self, key: String, slot: usize) -> Option<usize> {
        #[cfg(test)]
        self.record(|work| work.map_operations += 1);
        self.by_path.insert(key, slot)
    }

    pub(crate) fn replace(&mut self, entry: Entry) {
        if let Some(previous) = self.index_insert(entry.lookup_key().to_owned(), self.slots.len()) {
            self.slots[previous] = None;
        }
        self.slots.push(Some(entry));
        #[cfg(test)]
        self.record(|work| work.appended_slots += 1);
    }

    pub(crate) fn ensure_parents(&mut self, path: &name_policy::PortablePath) -> Result<()> {
        let mut parents = Vec::new();
        let mut parent = path.parent();
        while let Some(path) = parent {
            parent = path.parent();
            parents.push(path);
        }
        for path in parents.into_iter().rev() {
            if let Some(existing) = self.get_key(path.key()) {
                if existing.kind != NamespaceEntryKind::Directory {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ParentNotDirectory,
                        "parent is not a directory",
                    ));
                }
            } else {
                self.replace(Entry {
                    entry_id: new_id("entry"),
                    path,
                    kind: NamespaceEntryKind::Directory,
                    file_id: String::new(),
                    version_id: String::new(),
                    size: 0,
                });
            }
        }
        Ok(())
    }

    /// Preserve mutation order, not lookup-key order. These entries must pass
    /// `validate_entries` before becoming a published `NamespaceTree`.
    pub(crate) fn into_entries(self) -> Vec<Entry> {
        #[cfg(test)]
        self.record(|work| work.flattened_slots += self.slots.len());
        self.slots.into_iter().flatten().collect()
    }

    #[cfg(test)]
    fn record(&self, update: impl FnOnce(&mut Work)) {
        let mut work = self.work.get();
        update(&mut work);
        self.work.set(work);
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/business/namespace/draft_tests.rs"]
mod tests;
