//! A selection-of-existing-body merge has no metadata node representation.
use crate::business::file_tags::invalid;
use crate::engine::CurrentNamespaces;
use crate::{Result, State};

impl CurrentNamespaces {
    pub(super) fn verify_merge_metadata(&self, previous: &State, next: &State) -> Result<()> {
        if !previous.file_metadata_nodes.is_empty() || !next.file_metadata_nodes.is_empty() {
            return Err(invalid());
        }
        Ok(())
    }
}
