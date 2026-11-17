//! A saved working body pins its exact original immutable base, not every
//! version in the same FILE or history. Missing physical material cannot remove
//! this logical reference; only an authenticated working transition can do so.
use crate::{Result, State};

pub(crate) fn require_unpinned_working(
    state: &State,
    unavailable: impl Fn(&str) -> bool,
) -> Result<()> {
    for record in state.file_containers.values() {
        let Some((base, _)) = record.working.draft() else {
            continue;
        };
        // Full uploads and range overlays have the same captured base. A path
        // or metadata node remains a live selection even after HEAD advances.
        if unavailable(&base.version_id().to_string())
            || super::node_version(state, &base.node_id().to_string())
                .is_some_and(&unavailable)
        {
            return Err(super::pinned());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/node_retirement_working_tests.rs"]
mod tests;
