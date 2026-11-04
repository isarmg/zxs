//! Explicit fault-injection control, never a production offline API.
use super::Engine;
use crate::{
    Result,
    disk::{DiskIoBinding, DiskIoLease},
};

pub struct DiskQuiesceFault {
    binding: DiskIoBinding,
    control: Option<DiskIoLease>,
}

impl DiskQuiesceFault {
    /// End the fault owner's control responsibility before asserting full drain.
    pub fn release_control(&mut self) {
        self.control = None;
    }

    /// Whether this fault's ordinary and control responsibilities are gone.
    /// This is not an offline completion or a persisted membership decision.
    ///
    /// # Errors
    /// Returns RecoveryRequired if responsibility accounting is unavailable.
    pub fn is_drained(&self) -> Result<bool> {
        Ok(self.binding.status()?.drained())
    }

    /// # Errors
    /// Returns RecoveryRequired if responsibility accounting is unavailable.
    pub fn ordinary_count(&self) -> Result<usize> {
        Ok(self.binding.status()?.ordinary)
    }
}

impl Engine {
    /// Quiesce one disk's I/O gate for feature-gated fault/transport tests only.
    /// It does not change membership, persisted configuration or disk data.
    ///
    /// # Errors
    /// Rejects unavailable disks, stale authority or failed responsibility state.
    pub fn inject_disk_quiesce(&self, slot: &str) -> Result<DiskQuiesceFault> {
        let disk = self.disk(slot)?;
        let binding = self.disk_resources.get(disk)?.root.io_binding()?.clone();
        let control = binding
            .begin_quiesce()?
            .map(|mut transition| transition.take_control(&binding))
            .transpose()?;
        Ok(DiskQuiesceFault { binding, control })
    }
}
