//! Only outstanding preserving FILE exchanges, measured on their original
//! held volume. Space observations never decide whether a pair is admissible.
use super::{CapacityError, Census, EntryCost, Needed, Space};
use crate::{
    Result, database_install::OriginalFileStage, disk::DiskRoot,
    history_package::preparation_checkpoint,
};
use std::sync::atomic::AtomicBool;

pub(crate) struct BackupOriginalFileSpace<'a> {
    volume: Census<'a>,
    device: u64,
}

impl<'a> BackupOriginalFileSpace<'a> {
    pub(crate) fn capture(root: &'a DiskRoot) -> Result<Self> {
        Ok(Self {
            volume: Census::new(root)?,
            device: root.directory_identity(root.path())?.device,
        })
    }

    pub(crate) fn add_history(
        &mut self, history: &DiskRoot, mut stages: Vec<OriginalFileStage>, stop: &AtomicBool,
    ) -> Result<()> {
        // The caller opens the required history below its held same-mount
        // lifecycle root. Never count a missing history as a creatable parent.
        let mut census = Census::new(history)?;
        if history.directory_identity(history.path())?.device != self.device
            || census.unit != self.volume.unit
        {
            return Err(CapacityError::Geometry.into());
        }
        stages.sort_unstable_by(|a, b| a.generation.as_path().cmp(b.generation.as_path()));
        for stage in stages {
            preparation_checkpoint(stop)?;
            census.named_entry(stage.generation, EntryCost::Directory)?;
            // A generation may contain orphan attempts; a fresh attempt never
            // adopts them. EXCHANGE retains the old inode and allocates no copy.
            census.needed.charge(EntryCost::Directory, census.unit)?;
            census.needed.charge(EntryCost::File(stage.payload), census.unit)?;
            census.needed.charge(EntryCost::File(OriginalFileStage::INTENT_LIMIT), census.unit)?;
        }
        // A locally reported limit must fit too. Then combine all histories: separately
        // fitting paths may still exhaust the original volume together.
        census.require_with_reserve(0)?;
        add(&mut self.volume.needed, census.needed)
    }

    pub(crate) fn require(
        self, retained_floor: u64, completion_floor: u64, reserve: u64,
        maximum: u64,
    ) -> Result<()> {
        // The caller separately verifies any existing local FILE completion
        // and skips this entire new-work budget for that exact root.
        let mut local = Needed::default();
        local.charge(EntryCost::File(maximum), self.volume.unit)?;
        let latest = Space::observe(self.volume.root)?;
        if latest.unit != self.volume.unit {
            return Err(CapacityError::Geometry.into());
        }
        require_counts(
            self.volume.needed, retained_floor, local, completion_floor, reserve, &latest,
        )
    }
}

fn add(total: &mut Needed, part: Needed) -> Result<()> {
    total.bytes = total.bytes.checked_add(part.bytes).ok_or(CapacityError::Overflow)?;
    total.files = total.files.checked_add(part.files).ok_or(CapacityError::Overflow)?;
    total.directories = total.directories.checked_add(part.directories)
        .ok_or(CapacityError::Overflow)?;
    Ok(())
}

fn require_counts(
    mut material: Needed, retained_floor: u64, mut control: Needed, completion_floor: u64,
    reserve: u64, space: &Space,
) -> Result<()> {
    material.bytes = material.bytes.max(retained_floor);
    control.bytes = control.bytes.max(completion_floor);
    add(&mut material, control)?;
    material.require(space, reserve)
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/backup_original_file_space_tests.rs"]
mod tests;
