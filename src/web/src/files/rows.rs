//! Immutable bounded-page projections. Ordering and filtering belong to the
//! server before pagination; selection never rebuilds display metadata.
use super::model::{entry_key, file_kind};
use super::page::Entry;
use crate::ui::State;
use std::sync::Arc;
use web_interface::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct RowKey {
    context: Arc<RowContext>,
    entry_id: String,
    path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RowContext {
    epoch: u64,
    slot: String,
    directory: String,
}

#[derive(Clone, Debug)]
pub(super) struct Row {
    pub key: RowKey,
    pub check_id: String,
    pub name: String,
    pub directory: bool,
    pub selection_id: String,
    pub size: Option<u64>,
}
impl Row {
    pub fn current_context(&self, state: &State) -> bool {
        state.session_active()
            && state.session_epoch == self.key.context.epoch
            && state.files.slot == self.key.context.slot
            && state.files.directory == self.key.context.directory
    }

    pub fn current_entry<'a>(&self, state: &'a State) -> Option<&'a Entry> {
        if !self.current_context(state) {
            return None;
        }
        // Event-time identity lookup intentionally reads the current snapshot.
        // Rows hold display data only, never old entries or whole listing snapshots.
        state
            .files
            .entries
            .iter()
            .find(|entry| entry.entry_id() == self.key.entry_id && entry.path() == self.key.path)
    }
}
impl PartialEq for Row {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
            && self.name == other.name
            && self.selection_id == other.selection_id
            && self.directory == other.directory
            && self.size == other.size
    }
}

impl Eq for Row {}

#[derive(Clone, PartialEq)]
struct Listing {
    epoch: u64,
    slot: String,
    directory: String,
    entries: Arc<[Entry]>,
}
impl Listing {
    fn capture(state: &State) -> Self {
        Self {
            epoch: state.session_epoch,
            slot: state.files.slot.clone(),
            directory: state.files.directory.clone(),
            entries: state.files.entries.clone(),
        }
    }
    fn project(&self) -> Vec<Arc<Row>> {
        #[cfg(test)]
        tests::record_project(self.entries.len());
        let context = Arc::new(RowContext {
            epoch: self.epoch,
            slot: self.slot.clone(),
            directory: self.directory.clone(),
        });
        self.entries
            .iter()
            .map(|entry| {
                let check_id = entry_key(entry);
                let name = entry.name().to_owned();
                Arc::new(Row {
                    key: RowKey {
                        context: context.clone(),
                        entry_id: entry.entry_id().to_owned(),
                        path: entry.path().to_owned(),
                    },
                    check_id,
                    name,
                    directory: !file_kind(entry),
                    selection_id: entry_key(entry),
                    size: Some(entry.size()),
                })
            })
            .collect()
    }
}

pub(super) fn visible_rows(state: RwSignal<State>) -> Memo<Vec<Arc<Row>>> {
    // Arc equality short-circuits identical allocations. An unrelated State
    // notification does not traverse or clone any entry metadata.
    let listing = Memo::new(move |_| state.with(Listing::capture));
    Memo::new(move |_| listing.with(Listing::project))
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/rows/tests.rs"]
mod tests;
