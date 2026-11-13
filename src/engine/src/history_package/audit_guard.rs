//! Fail closed at package export until its format carries permanent path audit.
//! This is not publication admission or a portable-audit implementation. Native
//! copy, normal publication and histories without path facts remain unchanged.
use crate::{Error, PathChangeError, Result, State};
use crate::business::history_copy::retained;
use crate::state_projection::ProjectionSnapshot;
use std::collections::BTreeSet;
use storage_contract::history_window::HistoryFileId;

pub(super) fn require_representable(state: &State, history: &str) -> Result<()> {
    // Admit the actual native envelopes and imported references. The public
    // path visitor deliberately excludes retained source facts, so it alone
    // cannot decide whether exporting this history would silently lose audit.
    let mut native = false;
    crate::persist::path_changes::visit_ledger(state, || Ok(()), |fact| {
        native |= fact.history_tree_id.to_string() == history;
        Ok(())
    })?;
    if native {
        return Err(Error::PathChange(PathChangeError::PackageAuditUnsupported));
    }
    // Branch ownership also covers pending/borrowed bodies that do not yet
    // have a version owned by this file in the selected history.
    let files = state
        .versions
        .values()
        .filter(|version| version.history_tree_id == history)
        .map(|version| version.file_id.as_str())
        .chain(state.branches.values()
            .filter(|branch| branch.history_tree_id == history)
            .filter_map(|branch| branch.file_id.as_deref()))
        .map(|file| Ok(HistoryFileId::parse(file)?))
        .collect::<Result<BTreeSet<_>>>()?;
    let projection = ProjectionSnapshot::new(state);
    for file in files {
        // This shared reader flattens native and copied/moved prefixes without
        // promoting an imported source operation to a new native event.
        if !retained::path_changes(projection.view(), &file)?.is_empty() {
            return Err(Error::PathChange(PathChangeError::PackageAuditUnsupported));
        }
    }
    Ok(())
}
