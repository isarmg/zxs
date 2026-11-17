//! Final ordinary serving writes per held selected target. All regular-file
//! costs are rounded separately, with a separate inode count. Neither a space
//! estimate nor an already present file is evidence of valid material.
use super::{CapacityError, Census, EntryCost, Needed, Space};
use crate::{Result, disk::DiskRoot};
use platform_io::fs_safety::EntryName;
use std::num::NonZeroU64;

pub(crate) struct BackupServingSpace<'a> {
    root: &'a DiskRoot,
    demand: Demand,
}
struct Demand {
    unit: NonZeroU64,
    material: Needed,
    raw: u64,
    largest: u64,
}
impl<'a> BackupServingSpace<'a> {
    pub(crate) fn capture(root: &'a DiskRoot, missing_directories: u64) -> Result<Self> {
        Ok(Self { root, demand: Demand::at(Space::observe(root)?.unit, missing_directories)? })
    }
    pub(crate) fn missing_file(&mut self, length: u64) -> Result<()> {
        self.demand.missing_file(length)
    }

    pub(crate) fn require<const N: usize>(
        self, planned_files: usize, reserve: u64, control_floor: u64,
        records: [(&str, usize); N],
    ) -> Result<()> {
        let controls = self.controls(records)?;
        // Free counts may change while walking a large set. A mounted
        // filesystem's geometry must not silently change between observations.
        let latest = Space::observe(self.root)?;
        if latest.unit != self.demand.unit {
            return Err(CapacityError::Geometry.into());
        }
        self.demand.require_counts(&latest, controls, planned_files, reserve, control_floor)
    }

    fn controls<const N: usize>(&self, records: [(&str, usize); N]) -> Result<Needed> {
        let mut controls = Census::new(self.root)?;
        if controls.unit != self.demand.unit {
            return Err(CapacityError::Geometry.into());
        }
        for (name, maximum) in records {
            let leaf = EntryName::new(name)?;
            let length = u64::try_from(maximum).map_err(|_| CapacityError::Overflow)?;
            controls.entry(&leaf.as_relative(), true, EntryCost::File(length))?;
        }
        Ok(controls.needed)
    }
}

impl Demand {
    fn at(unit: NonZeroU64, directories: u64) -> Result<Self> {
        let bytes = EntryCost::Directory.allocation(unit)?.checked_mul(directories)
            .ok_or(CapacityError::Overflow)?;
        Ok(Self { unit, material: Needed { files: 0, directories, bytes }, raw: 0, largest: 0 })
    }

    fn missing_file(&mut self, length: u64) -> Result<()> {
        self.material.charge(EntryCost::File(length), self.unit)?;
        self.raw = self.raw.checked_add(length).ok_or(CapacityError::Overflow)?;
        self.largest = self.largest.max(length);
        Ok(())
    }

    fn require_counts(
        self, space: &Space, controls: Needed, planned_files: usize, reserve: u64, control_floor: u64,
    ) -> Result<()> {
        let planned = u64::try_from(planned_files).map_err(|_| CapacityError::Overflow)?;
        let previous = planned.checked_mul(8192).and_then(|n| n.checked_add(self.raw))
            .and_then(|n| n.checked_add(self.largest))
            .ok_or(CapacityError::Overflow)?;
        let scratch = self.largest.div_ceil(self.unit.get()).checked_mul(self.unit.get())
            .ok_or(CapacityError::Overflow)?;
        let material = self.material.bytes.checked_add(scratch).ok_or(CapacityError::Overflow)?;
        let bytes = material.max(previous).checked_add(controls.bytes.max(control_floor))
            .ok_or(CapacityError::Overflow)?;
        let files = self.material.files.checked_add(controls.files).ok_or(CapacityError::Overflow)?;
        let directories = self.material.directories.checked_add(controls.directories)
            .ok_or(CapacityError::Overflow)?;
        Needed { files, directories, bytes }.require(space, reserve)
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/backup_serving_space_tests.rs"]
mod tests;
