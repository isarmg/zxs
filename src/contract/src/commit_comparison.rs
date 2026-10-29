//! Immutable commit comparison without copied entry payloads or wire values.
//! Commits no longer embed their entry lists; callers materialize each side
//! from its paged namespace root (full comparison is whole-namespace by
//! definition) and this type owns the compared records.
use crate::{Commit, Entry};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug)]
pub struct CommitComparison {
    left: Arc<Commit>,
    right: Arc<Commit>,
    left_entries: Vec<Entry>,
    right_entries: Vec<Entry>,
    changes: Vec<ChangeIndex>,
}
#[derive(Debug)]
enum ChangeIndex {
    Removed(usize),
    Modified { before: usize, after: usize },
    Added(usize),
}
#[derive(Debug)]
pub enum CommitChange<'a> {
    Removed { before: &'a Entry },
    Modified { before: &'a Entry, after: &'a Entry },
    Added { after: &'a Entry },
}
impl CommitComparison {
    /// Compare cached path keys and version/display identities in entry order.
    /// The entry lists are each commit's complete materialized namespace; the
    /// comparison owns the exact compared records.
    pub fn between(
        left: Arc<Commit>,
        left_entries: Vec<Entry>,
        right: Arc<Commit>,
        right_entries: Vec<Entry>,
    ) -> Self {
        let changes = {
            fn index(entries: &[Entry]) -> BTreeMap<&str, usize> {
                entries
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| (entry.lookup_key(), index))
                    .collect()
            }
            let left_index = index(&left_entries);
            let right_index = index(&right_entries);
            let mut changes = Vec::new();
            for (before, entry) in left_entries.iter().enumerate() {
                match right_index.get(entry.lookup_key()) {
                    None => changes.push(ChangeIndex::Removed(before)),
                    Some(&after)
                        if entry.version_id != right_entries[after].version_id
                            || entry.path() != right_entries[after].path() =>
                    {
                        changes.push(ChangeIndex::Modified { before, after });
                    }
                    _ => {}
                }
            }
            for (after, entry) in right_entries.iter().enumerate() {
                if !left_index.contains_key(entry.lookup_key()) {
                    changes.push(ChangeIndex::Added(after));
                }
            }
            changes
        };
        Self {
            left,
            right,
            left_entries,
            right_entries,
            changes,
        }
    }
    pub fn left_id(&self) -> &str {
        &self.left.commit_id
    }
    pub fn right_id(&self) -> &str {
        &self.right.commit_id
    }
    /// Indices are produced only by enumeration of these private immutable owners.
    pub fn changes(&self) -> impl DoubleEndedIterator<Item = CommitChange<'_>> + ExactSizeIterator {
        self.changes.iter().map(|change| match *change {
            ChangeIndex::Removed(before) => CommitChange::Removed {
                before: &self.left_entries[before],
            },
            ChangeIndex::Modified { before, after } => CommitChange::Modified {
                before: &self.left_entries[before],
                after: &self.right_entries[after],
            },
            ChangeIndex::Added(after) => CommitChange::Added {
                after: &self.right_entries[after],
            },
        })
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/commit_comparison.rs"]
mod tests;
