//! The ordinary stage has one SYSTEM intent and an all-target selector
//! barrier. Presence here only constrains what can still be started; it never
//! proves a local completion, published native authority or a serving image.
use super::{DiskRoot, LOCAL_COMPLETE, Result, SELECTED, invalid, preparation_path_exists};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Markers { None, Selected, LocalComplete }

pub(super) fn require_selection(root: &DiskRoot, expected: &[u8]) -> Result<()> {
    if root.read_bytes(&root.path().join(SELECTED), 8192)? != expected {
        return Err(invalid("ordinary Backup target lost its exact original selection"));
    }
    Ok(())
}

pub(super) fn markers(root: &DiskRoot, selection: &[u8], has_intent: bool) -> Result<Markers> {
    let selected = preparation_path_exists(root, &root.path().join(SELECTED))?;
    let complete = preparation_path_exists(root, &root.path().join(LOCAL_COMPLETE))?;
    match (has_intent, selected, complete) {
        (_, false, false) => Ok(Markers::None),
        (false, true, _) | (_, false, true) => {
            Err(invalid("ordinary Backup target marker has no original SYSTEM intent or target selection"))
        }
        (true, true, _) => {
            require_selection(root, selection)?;
            Ok(if complete { Markers::LocalComplete } else { Markers::Selected })
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Progress { Unselected, SelectedEmpty, Started, LocalComplete }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LocalWork { PrepareRemaining, VerifyOnly }

pub(super) fn require_barrier(targets: &[Progress]) -> Result<LocalWork> {
    if targets.contains(&Progress::Unselected)
        && targets.iter().any(|p| matches!(p, Progress::Started | Progress::LocalComplete))
    {
        return Err(invalid("ordinary Backup material already exists beyond an incomplete target selection set"));
    }
    Ok(if targets.iter().all(|p| *p == Progress::LocalComplete) {
        LocalWork::VerifyOnly
    } else {
        LocalWork::PrepareRemaining
    })
}

#[cfg(test)]
#[path = "../../../../../../../../../../../test/engine/unit/backup_ordinary_selection_barrier_tests.rs"]
mod tests;
