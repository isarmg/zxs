//! Non-authoritative disk observations detached from the single writer.
//!
//! Capture configuration and logical accounting while holding the engine;
//! filesystem/seal probes run later without blocking publication. Never use an
//! observation as admission for a write: ordinary I/O uses its admitted held
//! resources, while explicit probes describe currently registered names under
//! those same bindings' revocable I/O ownership.
use super::Engine;
use crate::{disk::*, *};
use std::collections::BTreeMap;

/// Current metadata identity probes, detached from the engine lock.
/// This is an observation, never admission for a write.
pub struct MetadataMemberProbe {
    metadata_integrity: std::sync::Arc<super::provenance::MetadataIntegrity>,
    config: ValidatedConfig,
    resources: std::sync::Arc<DiskResources>,
    members: Vec<ObservationDisk>,
}

impl MetadataMemberProbe {
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Verify the captured current voter identities at observation time.
    ///
    /// # Errors
    /// Returns the first unavailable or mismatched member, preserving its source.
    pub fn verify(&self) -> Result<()> {
        for disk in &self.members {
            open_observation(
                &self.config,
                &self.resources,
                disk,
                Some(&self.metadata_integrity),
            )?;
        }
        Ok(())
    }
}

pub struct DiskObservationPlan {
    metadata_integrity: std::sync::Arc<super::provenance::MetadataIntegrity>,
    config: ValidatedConfig,
    resources: std::sync::Arc<DiskResources>,
    disks: Vec<ObservationDisk>,
    used_bytes: BTreeMap<String, u64>,
}

struct ObservationDisk {
    config: DiskConfig,
    registration_mode: RegistrationMode,
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
            resources: std::sync::Arc::clone(&self.disk_resources),
            members: self
                .observation_disks()
                .into_iter()
                .filter(|disk| disk.config.metadata_member)
                .collect(),
        }
    }

    fn observation_disks(&self) -> Vec<ObservationDisk> {
        let configured = self
            .config
            .disks
            .iter()
            .map(|disk| (disk.disk_id.as_str(), disk))
            .collect::<BTreeMap<_, _>>();
        self.registered_disks()
            .map(|registered| {
                let mut config = (**configured
                    .get(registered.disk_id)
                    .expect("invariant: registered binding has a historical disk definition"))
                .clone();
                config.metadata_member = registered.metadata_member;
                ObservationDisk {
                    config,
                    registration_mode: registered.registration_mode,
                }
            })
            .collect()
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
                match used_bytes.get_mut(slot) {
                    Some(used) => *used = used.saturating_add(version.logical_size),
                    None => {
                        used_bytes.insert(slot.clone(), version.logical_size);
                    }
                }
            }
        }
        DiskObservationPlan {
            metadata_integrity: std::sync::Arc::clone(&self.metadata_integrity),
            config: self.config.clone(),
            resources: std::sync::Arc::clone(&self.disk_resources),
            disks: self.observation_disks(),
            used_bytes,
        }
    }

    /// Return complete statuses, or the first typed seal/capacity failure.
    /// For partial availability use `prepare_disk_observation().sample()`.
    pub fn disk_status(&self) -> Result<Vec<DiskStatus>> {
        self.prepare_disk_observation().status()
    }

    /// Open a current registered directory for read-only informational observations.
    /// Ownership stays revocable across engine-lock release and dedicated workers;
    /// callers receive no raw File or descriptor. This does not grant writes or
    /// change metadata-integrity state.
    ///
    /// # Errors
    /// Rejects Disabled/Detached bindings before any target lookup and preserves
    /// concrete seal, missing-root and identity errors for enabled bindings.
    pub fn open_disk_observation_directory(&self, slot: &str) -> Result<DiskObservationDirectory> {
        let disk = self.disk(slot)?;
        let registered = self
            .registered_disk(&disk.disk_id)
            .ok_or_else(|| Error::new(ErrorCode::SlotNotFound, "disk registration is detached"))?;
        let disk = ObservationDisk {
            config: disk.clone(),
            registration_mode: registered.registration_mode,
        };
        open_observation(&self.config, &self.disk_resources, &disk, None)
    }

    /// Sample filesystem capacity from a currently registered, admitted member.
    /// Multiple directories may share a filesystem; capacity is not an independent pool.
    pub fn disk_capacity(&self, slot: &str) -> Result<(u64, u64, u64)> {
        let disk = self.disk(slot)?;
        let registered = self
            .registered_disk(&disk.disk_id)
            .ok_or_else(|| Error::new(ErrorCode::SlotNotFound, "disk registration is detached"))?;
        let mut config = disk.clone();
        config.metadata_member = registered.metadata_member;
        let disk = ObservationDisk {
            config,
            registration_mode: registered.registration_mode,
        };
        let directory = open_observation(
            &self.config,
            &self.disk_resources,
            &disk,
            Some(&self.metadata_integrity),
        )?;
        directory.capacity()
    }
}

fn open_observation(
    config: &ValidatedConfig,
    resources: &DiskResources,
    observation: &ObservationDisk,
    integrity: Option<&super::provenance::MetadataIntegrity>,
) -> Result<DiskObservationDirectory> {
    let disk = &observation.config;
    if observation.registration_mode == RegistrationMode::Disabled {
        return Err(Error::new(
            ErrorCode::DiskUnavailable,
            format!("disk {} registration is disabled", disk.slot_id),
        ));
    }
    // A detached plan has metadata, not admission. Re-admit before the first
    // open/stat of a configured name, using its original physical binding gate.
    let handle = resources.get(disk)?;
    let lease = handle.root.admit_io()?;
    let observed = DiskObservationDirectory::open(lease, config, disk, &handle.seal);
    if disk.metadata_member
        && let (Some(integrity), Err(error)) = (integrity, &observed)
        && error.code() != ErrorCode::DiskUnavailable
    {
        // A logical stop is not a newly observed metadata fault. Real seal,
        // identity, decode and read failures retain the original fault scope.
        integrity.fault(error);
    }
    observed
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
        self.disks
            .iter()
            .map(|observation| {
                let disk = &observation.config;
                let directory = open_observation(
                    &self.config,
                    &self.resources,
                    observation,
                    Some(&self.metadata_integrity),
                );
                let online = directory.is_ok();
                let capacity = directory
                    .map_err(|source| DiskObservationFailure {
                        slot_id: disk.slot_id.clone(),
                        stage: DiskObservationStage::Seal,
                        source: Box::new(source),
                    })
                    .and_then(|directory| {
                        directory.capacity().map_err(|source| DiskObservationFailure {
                            slot_id: disk.slot_id.clone(),
                            stage: DiskObservationStage::Capacity,
                            source: Box::new(source),
                        })
                    });
                let online = online
                    && !matches!(&capacity, Err(error) if error.source.code() == ErrorCode::DiskUnavailable);
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
