//! Discovery is a bounded read, not authority to detach or retire a file.
use crate::{Error, ErrorCode, Result};
use crate::history_window::{HistoryFileId, HistoryRepositoryId, HistoryRoot, HistoryTreeId, HistoryVersionId};
use data_serialization::{Deserialize, Serialize};

pub const PAGE_SIZE: usize = 100;
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const MAX_NAME_BYTES: usize = 4096;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub snapshot: Option<HistoryRoot>,
    pub after_file: Option<HistoryFileId>,
}
impl Request {
    pub fn validate(&self) -> Result<()> {
        if self.snapshot.is_some() != self.after_file.is_some() { return Err(invalid()); }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct File {
    pub file_id: HistoryFileId,
    pub repository_id: HistoryRepositoryId,
    pub history_tree_id: HistoryTreeId,
    pub history_root: HistoryRoot,
    pub representative_version_id: HistoryVersionId,
    /// The name captured with this version, never an asserted current path.
    pub recorded_name: String,
    #[serialization(with = "crate::u64_string")]
    pub representative_size: u64,
    #[serialization(with = "crate::u64_string")]
    pub retained_versions: u64,
}
impl File {
    pub fn validate(&self) -> Result<()> {
        if self.recorded_name.is_empty() || self.recorded_name.len() > MAX_NAME_BYTES
            || self.recorded_name.chars().any(char::is_control)
            || self.retained_versions == 0 { return Err(invalid()); }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Page {
    pub snapshot: HistoryRoot,
    pub files: Vec<File>,
    pub next_after: Option<HistoryFileId>,
}
impl Page {
    pub fn validate(&self, request: &Request) -> Result<()> {
        request.validate()?;
        if request.snapshot.is_some_and(|snapshot| snapshot != self.snapshot)
            || self.files.len() > PAGE_SIZE
            || self.files.windows(2).any(|pair| pair[0].file_id >= pair[1].file_id)
            || self.files.iter().any(|file| request.after_file.as_ref().is_some_and(|after| file.file_id <= *after))
            || self.next_after.as_ref().is_some_and(|next| {
                self.files.len() != PAGE_SIZE || self.files.last().is_none_or(|file| file.file_id != *next)
            }) { return Err(invalid()); }
        for file in &self.files { file.validate()?; }
        Ok(())
    }
}
fn invalid() -> Error {
    Error::new(ErrorCode::InvalidRequest, "invalid detached-history discovery page or cursor")
}

#[cfg(test)]
#[path = "../../../test/contract/unit/detached_history_tests.rs"]
mod tests;
