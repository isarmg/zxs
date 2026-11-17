//! Prospective native layout only. An already recorded registration or
//! namespace feedback is never re-admitted by today's free-space observation.
use super::{CapacityError, Census, EntryCost, Needed, Space};
use crate::disk::DiskRoot;
use crate::engine::backup_preparation::RESERVE;
use crate::engine::fresh_layout::FreshLayoutError;
use crate::Result;
use std::path::PathBuf;

// Preserve the historical 8 KiB minimum per construction and staging-parent
// allowance even on small-block filesystems. A larger native allocation wins.
const MIN_DIRECTORY: u64 = 8192;

pub(crate) fn require_new_backup_directories(
    root: &DiskRoot, directories: u64, layout_limit: usize,
) -> Result<()> {
    require_directories(&Space::observe(root)?, directories, layout_limit)
}

fn require_directories(space: &Space, directories: u64, layout_limit: usize) -> Result<()> {
    let one = EntryCost::Directory.allocation(space.unit)?.max(MIN_DIRECTORY);
    let bytes = one.checked_mul(directories).ok_or(CapacityError::Overflow)?;
    let limit = u64::try_from(layout_limit).map_err(|_| CapacityError::Overflow)?;
    let reserve = RESERVE.checked_add(limit).and_then(|n| n.checked_add(MIN_DIRECTORY))
        .ok_or(CapacityError::Overflow)?;
    Needed { files: 0, directories, bytes }.require(space, reserve)
}

/// Upper bounds cover not-yet-assigned identities. Each original source must
/// fit its complete requested set, including missing *distinct* parents.
/// Only the existing canonical feedback reader can admit a late file.
pub(in crate::engine) fn require_new_feedback_capacity<const N: usize>(
    root: &DiskRoot, records: [(PathBuf, usize); N],
) -> Result<()> {
    feedback_census(root, records)?.require_with_reserve(RESERVE)
}

fn feedback_census<const N: usize>(
    root: &DiskRoot, mut records: [(PathBuf, usize); N],
) -> Result<Census<'_>> {
    let mut census = Census::new(root)?;
    records.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    for (relative, maximum) in records {
        let limit = u64::try_from(maximum).map_err(|_| CapacityError::Overflow)?;
        if census.file_present(relative, limit)? {
            return Err(FreshLayoutError::BackupRootUnrecorded.into());
        }
    }
    Ok(census)
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/backup_layout_capacity_tests.rs"]
mod tests;
