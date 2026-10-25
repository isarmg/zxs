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
    /// Admit even an unsorted source, but never resolve corrupt duplicates by
    /// silently choosing a first or last entry. No source entry is normalized.
    pub(crate) fn new(entries: Vec<Entry>) -> Result<Self> {
        let mut draft = Self {
            slots: Vec::with_capacity(entries.len()),
            by_path: BTreeMap::new(),
            #[cfg(test)]
            work: Default::default(),
        };
        for entry in entries {
            #[cfg(test)]
            draft.record(|work| work.source_visits += 1);
            Self::validate_entry(&entry)?;
            if draft
                .index_insert(entry.lookup_key.clone(), draft.slots.len())
                .is_some()
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::StateInvalid,
                    "namespace source has duplicate lookup keys",
                ));
            }
            draft.slots.push(Some(entry));
        }
        for entry in draft.slots.iter().flatten() {
            #[cfg(test)]
            draft.record(|work| work.source_visits += 1);
            if let Some((parent, _)) = entry.lookup_key.rsplit_once('/')
                && draft
                    .get_key(parent)
                    .is_none_or(|entry| entry.kind != NamespaceEntryKind::Directory)
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::StateInvalid,
                    "namespace source parent is missing or is not a directory",
                ));
            }
        }
        Ok(draft)
    }

    fn validate_entry(entry: &Entry) -> Result<()> {
        if name_policy::validate_path(&entry.path).is_err()
            || entry.lookup_key != name_policy::path_key(&entry.path)
            || entry.name != entry.path.rsplit('/').next().unwrap_or("")
        {
            return Err(Error::new(
                storage_contract::ErrorCode::StateInvalid,
                "namespace entry path, key or name is invalid",
            ));
        }
        Ok(())
    }

    /// Lookup is intentionally not create-path validation: folded and NFD
    /// query spellings can resolve an existing canonical display path.
    pub(crate) fn get(&self, path: &str) -> Option<&Entry> {
        self.get_key(&name_policy::path_key(path))
    }

    fn get_key(&self, key: &str) -> Option<&Entry> {
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

    pub(crate) fn replace(&mut self, entry: Entry) -> Result<()> {
        Self::validate_entry(&entry)?;
        if let Some(previous) = self.index_insert(entry.lookup_key.clone(), self.slots.len()) {
            self.slots[previous] = None;
        }
        self.slots.push(Some(entry));
        #[cfg(test)]
        self.record(|work| work.appended_slots += 1);
        Ok(())
    }

    pub(crate) fn ensure_parents(&mut self, path: &str) -> Result<()> {
        let parts = path.split('/').collect::<Vec<_>>();
        let mut parent = String::new();
        for component in parts.iter().take(parts.len().saturating_sub(1)) {
            if !parent.is_empty() {
                parent.push('/');
            }
            parent.push_str(component);
            if let Some(existing) = self.get(&parent) {
                if existing.kind != NamespaceEntryKind::Directory {
                    return Err(Error::new(
                        storage_contract::ErrorCode::ParentNotDirectory,
                        "parent is not a directory",
                    ));
                }
            } else {
                self.replace(Entry {
                    entry_id: new_id("entry"),
                    path: parent.clone(),
                    name: (*component).into(),
                    lookup_key: name_policy::path_key(&parent),
                    kind: NamespaceEntryKind::Directory,
                    file_id: String::new(),
                    version_id: String::new(),
                    size: 0,
                })?;
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
