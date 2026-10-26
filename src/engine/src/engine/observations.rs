//! Non-authoritative disk observations detached from the single writer.
//!
//! Capture configuration and logical accounting while holding the engine;
//! filesystem/seal probes run later without blocking publication. Never use an
//! observation as admission for a write: commit paths verify current bindings.
use super::Engine;
use crate::{disk::*, *};
use platform_io::fs_safety::linux::{MountPolicy, OpenAt2Root};
use std::collections::BTreeMap;

pub struct DiskObservationPlan {
    config: Config,
    used_bytes: BTreeMap<String, u64>,
}

/// One non-authoritative observation, never admission for a write.
pub struct DiskObservation {
    pub status: DiskStatus,
    /// Physical total, available and used bytes from the same status probe.
    pub capacity: Option<(u64, u64, u64)>,
}

impl Engine {
    pub fn prepare_disk_observation(&self) -> DiskObservationPlan {
        let mut used_bytes: BTreeMap<String, u64> = BTreeMap::new();
        for placement in self.state.placements.values() {
            if self
                .state
                .content_states
                .get(&placement.version_id)
                .is_some_and(|state| {
                    *state == ContentState::Purged || *state == ContentState::Migrated
                })
            {
                continue;
            }
            let version = self
                .state
                .versions
                .get(&placement.version_id)
                .expect("invariant: every placement references an existing version");
            for (index, slot) in placement.route_slot_ids.iter().enumerate() {
                // Match used_bytes' membership semantics even for duplicate slots.
                if placement.route_slot_ids[..index].contains(slot) {
                    continue;
                }
                let used = used_bytes.entry(slot.clone()).or_default();
                *used = used.saturating_add(version.logical_size);
            }
        }
        DiskObservationPlan {
            config: self.config.clone(),
            used_bytes,
        }
    }

    pub fn disk_status(&self) -> Result<Vec<DiskStatus>> {
        self.prepare_disk_observation().status()
    }

    /// Filesystem capacity is sampled from the actual managed member mount.
    /// Development directories may share one physical filesystem.
    pub fn disk_capacity(&self, slot: &str) -> Result<(u64, u64, u64)> {
        let disk = self.disk(slot)?;
        capacity(disk, &validate_seal(&self.config, disk)?)
    }
}

fn capacity(disk: &DiskConfig, seal: &DiskSeal) -> Result<(u64, u64, u64)> {
    // Pin the validated member, rather than sampling an empty mountpoint on
    // the system filesystem after a missing/unmounted data disk.
    let directory = OpenAt2Root::open(&disk.root, MountPolicy::SameMount).map_err(|error| {
        let message = error.to_string();
        Error::from(error).context(ErrorCode::DiskIdentityMismatch, message)
    })?;
    if directory.identity().device != seal.device || directory.identity().inode != seal.inode {
        return Err(Error::new(
            storage_contract::ErrorCode::DiskIdentityMismatch,
            "disk root changed during capacity observation",
        ));
    }
    let stat = rustix::fs::fstatvfs(directory.directory()).map_err(std::io::Error::from)?;
    let total = stat.f_blocks.saturating_mul(stat.f_frsize);
    let available = stat.f_bavail.saturating_mul(stat.f_frsize);
    let used = stat
        .f_blocks
        .saturating_sub(stat.f_bfree)
        .saturating_mul(stat.f_frsize);
    Ok((total, available, used))
}

impl DiskObservationPlan {
    /// Observe physical availability using the captured binding configuration.
    pub fn status(&self) -> Result<Vec<DiskStatus>> {
        Ok(self
            .sample()
            .into_iter()
            .map(|observation| observation.status)
            .collect())
    }

    /// Sample each binding once; returned capacities are observations, not later resamples.
    pub fn sample(&self) -> Vec<DiskObservation> {
        self.config
            .disks
            .iter()
            .map(|disk| {
                let seal = validate_seal(&self.config, disk).ok();
                let capacity = seal.as_ref().and_then(|seal| capacity(disk, seal).ok());
                let status = DiskStatus {
                    slot_id: disk.slot_id.clone(),
                    disk_id: disk.disk_id.clone(),
                    level: disk.level,
                    upstream_slot_id: disk.upstream_slot_id.clone(),
                    online: seal.is_some(),
                    metadata_member: disk.metadata_member,
                    physical_device: seal.map(|seal| seal.physical_device).unwrap_or_default(),
                    available_bytes: capacity.map(|capacity| capacity.1).unwrap_or(0),
                    used_bytes: self.used_bytes.get(&disk.slot_id).copied().unwrap_or(0),
                    simulation: self.config.mode == Mode::Development,
                };
                DiskObservation { status, capacity }
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/observations/tests.rs"]
mod tests;
