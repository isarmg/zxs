//! Complete comparison responses encoded once from immutable domain records.
use crate::{
    ApiError, HistoryReadControl,
    backend_file_history::{BoundedWriter, EncodedPage},
};
use data_serialization::Serialize;
use storage_contract::{CommitChange, CommitComparison, Entry};

pub struct CompareCommits<'a> {
    pub left: &'a str,
    pub right: &'a str,
}

/// Complete bounded bytes; owns no publication and grants no future authority.
#[derive(Debug)]
pub struct ComparisonSnapshot {
    encoded: EncodedPage,
}
impl ComparisonSnapshot {
    /// Encode every change once, or reject the complete response above 8 MiB.
    /// The input comparison's whole-namespace memory is outside this wire budget.
    pub fn encode(
        comparison: &CommitComparison,
        control: &HistoryReadControl,
    ) -> Result<Self, ApiError> {
        Self::encode_with_limit(
            comparison,
            control,
            storage_contract::client_protocol::MAX_RESPONSE_BYTES,
        )
    }

    fn encode_with_limit(
        comparison: &CommitComparison,
        control: &HistoryReadControl,
        limit: usize,
    ) -> Result<Self, ApiError> {
        let mut writer = BoundedWriter::new(limit);
        checkpoint(control)?;
        writer.punctuation(b"{\"left\":")?;
        writer.json(&comparison.left_id())?;
        writer.punctuation(b",\"right\":")?;
        writer.json(&comparison.right_id())?;
        writer.punctuation(b",\"changes\":[")?;
        for (index, change) in comparison.changes().enumerate() {
            checkpoint(control)?;
            if index != 0 {
                writer.punctuation(b",")?;
            }
            writer.json(&ChangeRecord::from(change))?;
        }
        writer.punctuation(b"]}")?;
        checkpoint(control)?;
        Ok(Self {
            encoded: writer.finish(),
        })
    }

    pub(crate) fn into_encoded(self) -> EncodedPage {
        self.encoded
    }

    #[cfg(test)]
    pub(crate) fn body(&self) -> &[u8] {
        self.encoded.body()
    }
}

fn checkpoint(control: &HistoryReadControl) -> Result<(), ApiError> {
    control.checkpoint().map_err(|source| {
        ApiError::new(
            ::http::StatusCode::REQUEST_TIMEOUT,
            source.code(),
            source.message(),
        )
        .with_source(source)
    })
}

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
impl<'a> From<CommitChange<'a>> for ChangeRecord<'a> {
    fn from(change: CommitChange<'a>) -> Self {
        match change {
            CommitChange::Removed { before } => Self::Removed {
                path: before.path(),
                before,
            },
            CommitChange::Modified { before, after } => Self::Modified {
                path: after.path(),
                before,
                after,
            },
            CommitChange::Added { after } => Self::Added {
                path: after.path(),
                after,
            },
        }
    }
}
#[cfg(test)]
#[path = "../../../test/server/unit/backend_comparison.rs"]
mod tests;
