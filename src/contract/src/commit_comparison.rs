//! Immutable commit comparison without copied entry payloads or wire values.
use crate::{Commit, Entry};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug)]
pub struct CommitComparison {
    left: Arc<Commit>,
    right: Arc<Commit>,
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
    /// Handles keep the exact compared records alive independently of their owner.
    pub fn between(left: Arc<Commit>, right: Arc<Commit>) -> Self {
        let changes = {
            fn index(entries: &[Entry]) -> BTreeMap<&str, usize> {
                entries
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| (entry.lookup_key(), index))
                    .collect()
            }
            let left_entries = index(&left.entries);
            let right_entries = index(&right.entries);
            let mut changes = Vec::new();
            for (before, entry) in left.entries.iter().enumerate() {
                match right_entries.get(entry.lookup_key()) {
                    None => changes.push(ChangeIndex::Removed(before)),
                    Some(&after)
                        if entry.version_id != right.entries[after].version_id
                            || entry.path() != right.entries[after].path() =>
                    {
                        changes.push(ChangeIndex::Modified { before, after });
                    }
                    _ => {}
                }
            }
            for (after, entry) in right.entries.iter().enumerate() {
                if !left_entries.contains_key(entry.lookup_key()) {
                    changes.push(ChangeIndex::Added(after));
                }
            }
            changes
        };
        Self {
            left,
            right,
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
                before: &self.left.entries[before],
            },
            ChangeIndex::Modified { before, after } => CommitChange::Modified {
                before: &self.left.entries[before],
                after: &self.right.entries[after],
            },
            ChangeIndex::Added(after) => CommitChange::Added {
                after: &self.right.entries[after],
            },
        })
    }
}

#[cfg(test)]
#[path = "../../../test/contract/unit/commit_comparison.rs"]
mod tests;
