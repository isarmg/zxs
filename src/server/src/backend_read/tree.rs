//! A complete tree response, encoded once within the client response budget.
use crate::{
    ApiError,
    backend_file_history::{BoundedWriter, EncodedPage},
};
use data_serialization::Serialize;
use std::collections::BTreeMap;
use storage_contract::{Entry, client_protocol::MAX_RESPONSE_BYTES};

/// Complete encoded tree data. Oversized trees fail rather than truncate.
/// This response owns no read view and grants no future authorization.
#[derive(Debug)]
pub struct TreeSnapshot {
    encoded: EncodedPage,
}

impl TreeSnapshot {
    /// Encode an adapter's entries using the same bounded encoder as engine reads.
    /// Returns `ResourceLimitExceeded` if the complete response exceeds 8 MiB.
    pub fn encode(
        branch: &str,
        commit: &str,
        entries: impl IntoIterator<Item = Entry>,
        pending: &BTreeMap<String, String>,
    ) -> Result<Self, ApiError> {
        let mut encoder = TreeEncoder::new(branch, commit)?;
        for entry in entries {
            encoder.entry(&entry)?;
        }
        encoder.finish(pending)
    }

    pub(crate) fn into_encoded(self) -> EncodedPage {
        self.encoded
    }

    #[cfg(test)]
    pub(crate) fn body(&self) -> &[u8] {
        self.encoded.body()
    }
}

pub(crate) struct TreeEncoder {
    writer: BoundedWriter,
    has_entries: bool,
}

impl TreeEncoder {
    pub(crate) fn new(branch: &str, commit: &str) -> Result<Self, ApiError> {
        let mut writer = BoundedWriter::new(MAX_RESPONSE_BYTES);
        writer.punctuation(b"{\"branch_id\":")?;
        writer.json(&branch)?;
        writer.punctuation(b",\"commit_id\":")?;
        writer.json(&commit)?;
        writer.punctuation(b",\"entries\":[")?;
        Ok(Self {
            writer,
            has_entries: false,
        })
    }

    pub(crate) fn entry(&mut self, entry: &Entry) -> Result<(), ApiError> {
        if self.has_entries {
            self.writer.punctuation(b",")?;
        }
        self.writer.json(entry)?;
        self.has_entries = true;
        Ok(())
    }

    pub(crate) fn finish(mut self, pending: &impl Serialize) -> Result<TreeSnapshot, ApiError> {
        self.writer.punctuation(b"],\"pending_file_versions\":")?;
        self.writer.json(pending)?;
        self.writer.punctuation(b"}")?;
        Ok(TreeSnapshot {
            encoded: self.writer.finish(),
        })
    }
}
