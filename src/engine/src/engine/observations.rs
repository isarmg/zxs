//! Non-authoritative disk observations detached from the single writer.
//!
//! Capture configuration and logical accounting while holding the engine;
//! filesystem/seal probes run later without blocking publication. Never use an
//! observation as admission for a write: ordinary I/O uses its admitted held
//! resources, while explicit probes describe the current configured names.
use super::Engine;
use crate::{disk::*, *};
use platform_io::fs_safety::linux::{MountPolicy, OpenAt2Root};
use std::collections::BTreeMap;

/// Current metadata identity probes, detached from the engine lock.
/// This is an observation, never admission for a write.
pub struct MetadataMemberProbe {
    metadata_integrity: std::sync::Arc<super::provenance::MetadataIntegrity>,
    config: ValidatedConfig,
}

impl MetadataMemberProbe {
    pub fn member_count(&self) -> usize {
        self.config.metadata_members().len()
    }

    /// Verify all configured metadata identities at observation time.
    ///
    /// # Errors
    /// Returns the first unavailable or mismatched member, preserving its source.
    pub fn verify(&self) -> Result<()> {
        for disk in self.config.metadata_members() {
            self.metadata_integrity
                .observe(validate_seal(&self.config, disk))?;
        }
        Ok(())
    }
}

pub struct DiskObservationPlan {
    metadata_integrity: std::sync::Arc<super::provenance::MetadataIntegrity>,
    config: ValidatedConfig,
    used_bytes: BTreeMap<String, u64>,
}

/// One non-authoritative observation, never admission for a write.
pub struct DiskObservation {
    pub status: ObservedDiskStatus,
    /// Physical total, available and used bytes from the same status probe.
    pub capacity: std::result::Result<(u64, u64, u64), DiskObservationFailure>,
}

/// Binding and logical accounting, independent of physical capacity availability.
#[derive(Debug)]
pub struct ObservedDiskStatus {
    pub slot_id: String,
    pub disk_id: String,
    pub level: u32,
    pub upstream_slot_id: Option<String>,
    pub online: bool,
    pub metadata_member: bool,
    pub physical_device: Option<String>,
    pub used_bytes: u64,
    pub simulation: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiskObservationStage {
    Seal,
    Capacity,
}

/// A failed sample retains its phase and concrete cause, not a zero-byte sample.
#[derive(Debug, error_definition::Error)]
#[error("disk {slot_id} observation failed at {stage:?}: {source}")]
pub struct DiskObservationFailure {
    pub slot_id: String,
    pub stage: DiskObservationStage,
    #[source]
    pub source: Box<Error>,
}

impl DiskObservation {
    fn into_status(self) -> Result<DiskStatus> {
        let capacity = self.capacity.map_err(Error::DiskObservation)?;
        let ObservedDiskStatus {
            slot_id,
            disk_id,
            level,
            upstream_slot_id,
            online,
            metadata_member,
            physical_device,
            used_bytes,
            simulation,
        } = self.status;
        Ok(DiskStatus {
            slot_id,
            disk_id,
            level,
            upstream_slot_id,
            online,
            metadata_member,
            physical_device,
            available_bytes: Some(capacity.1),
            observation_failure: None,
            used_bytes,
            simulation,
        })
    }
}

impl Engine {
    /// Capture shared immutable configuration without inspecting historical state.
    pub fn prepare_metadata_member_probe(&self) -> MetadataMemberProbe {
        MetadataMemberProbe {
            metadata_integrity: std::sync::Arc::clone(&self.metadata_integrity),
            config: self.config.clone(),
        }
    }

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
            metadata_integrity: std::sync::Arc::clone(&self.metadata_integrity),
            config: self.config.clone(),
            used_bytes,
        }
    }

    /// Return complete statuses, or the first typed seal/capacity failure.
    /// For partial availability use `prepare_disk_observation().sample()`.
    pub fn disk_status(&self) -> Result<Vec<DiskStatus>> {
        self.prepare_disk_observation().status()
    }

    /// Open the current registered directory for read-only informational observations.
    /// The returned descriptor is the same directory object whose seal was verified;
    /// callers must inspect that descriptor rather than reopening the configured path.
    /// This does not grant write admission or change metadata-integrity state.
    ///
    /// # Errors
    /// Preserves missing-root, seal and identity errors without substituting the
    /// filesystem underneath an unavailable or replaced mountpoint.
    pub fn open_disk_observation_directory(&self, slot: &str) -> Result<std::fs::File> {
        let disk = self.disk(slot)?;
        let observed = DiskHandle::open(&self.config, disk, true)?;
        observed.root.directory(&disk.root)
    }

    /// Filesystem capacity is sampled from the actual managed member mount.
    /// Multiple managed directories may share a filesystem; capacity is not an independent pool.
    pub fn disk_capacity(&self, slot: &str) -> Result<(u64, u64, u64)> {
        let disk = self.disk(slot)?;
        let identity = validate_seal(&self.config, disk);
        let identity = if disk.metadata_member {
            self.metadata_integrity.observe(identity)?
        } else {
            identity?
        };
        capacity(disk, &identity).inspect_err(|error| {
            if disk.metadata_member && error.code() == ErrorCode::DiskIdentityMismatch {
                self.metadata_integrity.fault(error);
            }
        })
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
    filesystem_capacity(directory.directory())
}

fn filesystem_capacity(directory: &std::fs::File) -> Result<(u64, u64, u64)> {
    let stat = rustix::fs::fstatvfs(directory).map_err(std::io::Error::from)?;
    let total = stat.f_blocks.saturating_mul(stat.f_frsize);
    let available = stat.f_bavail.saturating_mul(stat.f_frsize);
    let used = stat
        .f_blocks
        .saturating_sub(stat.f_bfree)
        .saturating_mul(stat.f_frsize);
    Ok((total, available, used))
}

impl DiskObservationPlan {
    /// Observe managed-directory availability using the captured binding configuration.
    ///
    /// # Errors
    /// Returns the first seal/capacity failure; an unknown capacity is not zero.
    pub fn status(&self) -> Result<Vec<DiskStatus>> {
        self.sample()
            .into_iter()
            .map(DiskObservation::into_status)
            .collect()
    }

    /// Sample each binding once; returned capacities are observations, not later resamples.
    pub fn sample(&self) -> Vec<DiskObservation> {
        self.config
            .disks
            .iter()
            .map(|disk| {
                let identity = validate_seal(&self.config, disk);
                let seal = if disk.metadata_member {
                    self.metadata_integrity.observe(identity)
                } else {
                    identity
                };
                let online = seal.is_ok();
                let capacity = seal
                    .map_err(|source| DiskObservationFailure {
                        slot_id: disk.slot_id.clone(),
                        stage: DiskObservationStage::Seal,
                        source: Box::new(source),
                    })
                    .and_then(|seal| {
                        capacity(disk, &seal)
                            .inspect_err(|error| {
                                if disk.metadata_member
                                    && error.code() == ErrorCode::DiskIdentityMismatch
                                {
                                    self.metadata_integrity.fault(error);
                                }
                            })
                            .map_err(|source| DiskObservationFailure {
                                slot_id: disk.slot_id.clone(),
                                stage: DiskObservationStage::Capacity,
                                source: Box::new(source),
                            })
                    });
                let status = ObservedDiskStatus {
                    slot_id: disk.slot_id.clone(),
                    disk_id: disk.disk_id.clone(),
                    level: disk.level,
                    upstream_slot_id: disk.upstream_slot_id.clone(),
                    online,
                    metadata_member: disk.metadata_member,
                    physical_device: None,
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
