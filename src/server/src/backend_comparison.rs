//! Borrowed comparison wire records over immutable typed domain snapshots.
use data_serialization::{Serialize, Serializer, ser::SerializeSeq};
use storage_contract::{CommitChange, CommitComparison, Entry};

pub struct CompareCommits<'a> {
    pub left: &'a str,
    pub right: &'a str,
}
pub(crate) struct ComparisonResponse(CommitComparison);
impl From<CommitComparison> for ComparisonResponse {
    fn from(comparison: CommitComparison) -> Self {
        Self(comparison)
    }
}
#[derive(Serialize)]
struct ComparisonRecord<'a> {
    left: &'a str,
    right: &'a str,
    changes: Changes<'a>,
}
struct Changes<'a>(&'a CommitComparison);
#[derive(Serialize)]
#[serialization(tag = "change", rename_all = "lowercase")]
enum ChangeRecord<'a> {
    Removed {
        path: &'a str,
        before: &'a Entry,
    },
    Modified {
        path: &'a str,
        before: &'a Entry,
        after: &'a Entry,
    },
    Added {
        path: &'a str,
        after: &'a Entry,
    },
}
impl Serialize for Changes<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let changes = self.0.changes();
        let mut sequence = serializer.serialize_seq(Some(changes.len()))?;
        for change in changes {
            let record = match change {
                CommitChange::Removed { before } => ChangeRecord::Removed {
                    path: before.path(),
                    before,
                },
                CommitChange::Modified { before, after } => ChangeRecord::Modified {
                    path: after.path(),
                    before,
                    after,
                },
                CommitChange::Added { after } => ChangeRecord::Added {
                    path: after.path(),
                    after,
                },
            };
            sequence.serialize_element(&record)?;
        }
        sequence.end()
    }
}
impl Serialize for ComparisonResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        ComparisonRecord {
            left: self.0.left_id(),
            right: self.0.right_id(),
            changes: Changes(&self.0),
        }
        .serialize(serializer)
    }
}
#[cfg(test)]
#[path = "../../../test/server/unit/backend_comparison.rs"]
mod tests;
