//! Keep the existing first retained Version -> history -> repository authority.
//! A page is one read of the current state, not up to 65 complete Version scans.
use super::{ApiError, Application, repo_for_history};
use std::collections::BTreeSet;
use storage_contract::{
    FileId,
    automatic_observation::{PAGE_LIMIT, Page},
};

impl Application<'_> {
    fn admit_first_file_histories(
        &self,
        count: usize,
        take: impl FnMut(&str) -> bool,
    ) -> Result<(), ApiError> {
        let rows = self
            .engine
            .state()
            .versions
            .values()
            .map(|version| (version.file_id.as_str(), version.history_tree_id.as_str()));
        first_histories(rows, count, take, |history| {
            repo_for_history(self.engine, history).map(|_| ())
        })
    }

    pub(crate) fn authorize_file(&self, file: &str) -> Result<(), ApiError> {
        self.admit_first_file_histories(1, |candidate| candidate == file)
    }

    /// A returned continuation discloses that another real FILE follows. Its
    /// actual holder is checked even if it was not one of the returned rows.
    pub(in crate::engine_backend) fn authorize_automatic_page(
        &self,
        page: &Page,
        lookahead: Option<FileId>,
    ) -> Result<(), ApiError> {
        if page.rows.len() > PAGE_LIMIT || page.next.is_some() != lookahead.is_some() {
            return Err(ApiError::forbidden());
        }
        let mut needed: BTreeSet<String> = page
            .rows
            .iter()
            .map(|row| row.key.file)
            .chain(lookahead)
            .map(|file| file.to_string())
            .collect();
        self.admit_first_file_histories(needed.len(), |file| needed.remove(file))
    }
}

/// Process each requested FILE's first Version under precisely the original
/// iteration order. Never search for an alternative, easier-to-admit history.
/// No FILE IDs, selected histories or permission decisions outlive this read.
fn first_histories<'a>(
    rows: impl Iterator<Item = (&'a str, &'a str)>,
    mut left: usize,
    mut take: impl FnMut(&str) -> bool,
    mut admit: impl FnMut(&str) -> Result<(), ApiError>,
) -> Result<(), ApiError> {
    if left == 0 {
        return Ok(());
    }
    for (file, history) in rows {
        if take(file) {
            admit(history)?;
            left -= 1;
            if left == 0 {
                return Ok(());
            }
        }
    }
    Err(ApiError::new(
        ::http::StatusCode::NOT_FOUND,
        storage_contract::ErrorCode::FileNotFound,
        "file not found",
    ))
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/automatic_file_authority.rs"]
mod tests;
