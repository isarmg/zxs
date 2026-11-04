//! Current registration provenance, deliberately separate from physical health.
//!
//! Today only the Engine's admitted configuration supplies registrations. Its
//! supported generations preserve existing slot/disk/binding identities. This is
//! NOT a certified lifecycle registry: detach/rebind needs a new authority layer,
//! not a caller-provided set of supposedly registered slots.
use super::{Engine, ValidatedConfig};
use crate::DiskConfig;

/// Exact logical registration identity. A path is not a binding identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskRegistrationIdentity {
    pub pool_id: String,
    pub disk_id: String,
    pub slot_id: String,
    pub binding_generation: u64,
}

/// Borrowed metadata only; neither the disk root nor any I/O capability escapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisteredDisk<'a> {
    pub pool_id: &'a str,
    pub disk_id: &'a str,
    pub slot_id: &'a str,
    pub binding_generation: u64,
    pub level: u32,
    pub upstream_slot_id: Option<&'a str>,
    pub metadata_member: bool,
}
impl RegisteredDisk<'_> {
    pub fn identity(self) -> DiskRegistrationIdentity {
        DiskRegistrationIdentity {
            pool_id: self.pool_id.into(),
            disk_id: self.disk_id.into(),
            slot_id: self.slot_id.into(),
            binding_generation: self.binding_generation,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct CurrentDiskRegistry<'a> {
    config: &'a ValidatedConfig,
}
impl<'a> CurrentDiskRegistry<'a> {
    pub(super) fn from_engine(engine: &'a Engine) -> Self {
        Self {
            config: &engine.config,
        }
    }
    pub(super) fn generation_hash(self) -> Option<&'a str> {
        self.config.generation_hash()
    }
    pub(super) fn disks(self) -> impl ExactSizeIterator<Item = RegisteredDisk<'a>> {
        self.config.disks.iter().map(move |disk| self.project(disk))
    }
    pub(super) fn slot(self, slot: &str) -> Option<RegisteredDisk<'a>> {
        // Exact canonical placement identity, not a case-insensitive user lookup.
        self.config
            .disks
            .iter()
            .find(|disk| disk.slot_id == slot)
            .map(|disk| self.project(disk))
    }
    fn project(self, disk: &'a DiskConfig) -> RegisteredDisk<'a> {
        RegisteredDisk {
            pool_id: &self.config.pool_id,
            disk_id: &disk.disk_id,
            slot_id: &disk.slot_id,
            binding_generation: disk.binding_generation,
            level: disk.level,
            upstream_slot_id: disk.upstream_slot_id.as_deref(),
            metadata_member: disk.metadata_member,
        }
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/disk_deregistration_plan_tests.rs"]
mod tests;
