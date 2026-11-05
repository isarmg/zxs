//! Public administrator diagnostics, encoded only after backend work returns.
use data_serialization::Serialize;

#[derive(Serialize)]
pub(crate) struct DisksResponse {
    disks: Vec<DiskRecord>,
}
#[derive(Serialize)]
struct DiskRecord {
    slot_id: String,
    disk_id: String,
    level: u32,
    upstream_slot_id: Option<String>,
    online: bool,
    metadata_member: bool,
    physical_device: Option<String>,
    #[serialization(with = "storage_contract::optional_u64_string")]
    available_bytes: Option<u64>,
    observation_failure: Option<storage_contract::DiskObservationIssue>,
    #[serialization(with = "storage_contract::u64_string")]
    used_bytes: u64,
    simulation: bool,
}
impl From<storage_contract::DiskStatus> for DiskRecord {
    fn from(disk: storage_contract::DiskStatus) -> Self {
        let storage_contract::DiskStatus {
            slot_id,
            disk_id,
            level,
            upstream_slot_id,
            online,
            metadata_member,
            physical_device,
            available_bytes,
            observation_failure,
            used_bytes,
            simulation,
        } = disk;
        Self {
            slot_id,
            disk_id,
            level,
            upstream_slot_id,
            online,
            metadata_member,
            physical_device,
            available_bytes,
            observation_failure,
            used_bytes,
            simulation,
        }
    }
}
impl From<Vec<storage_contract::DiskStatus>> for DisksResponse {
    fn from(disks: Vec<storage_contract::DiskStatus>) -> Self {
        Self {
            disks: disks.into_iter().map(DiskRecord::from).collect(),
        }
    }
}
#[derive(Serialize)]
pub(crate) struct CheckResponse {
    checked_versions: usize,
    healthy_replicas: usize,
    tombstoned_versions: usize,
    issues: Vec<String>,
}
impl From<storage_contract::CheckReport> for CheckResponse {
    fn from(report: storage_contract::CheckReport) -> Self {
        let storage_contract::CheckReport {
            checked_versions,
            healthy_replicas,
            tombstoned_versions,
            issues,
        } = report;
        Self {
            checked_versions,
            healthy_replicas,
            tombstoned_versions,
            issues,
        }
    }
}
#[cfg(test)]
#[path = "../../../test/server/unit/backend_diagnostics.rs"]
mod tests;

#[derive(Serialize)]
pub(crate) struct FileManagerDisksResponse {
    disks: Vec<FileManagerDiskRecord>,
}
#[derive(Serialize)]
struct FileManagerDiskRecord {
    name: String,
    slot_id: String,
    level: u32,
    online: bool,
    writable: bool,
    #[serialization(with = "storage_contract::optional_u64_string")]
    available_bytes: Option<u64>,
    observation_failure: Option<storage_contract::DiskObservationIssue>,
    #[serialization(with = "storage_contract::u64_string")]
    used_bytes: u64,
    simulation: bool,
}
impl FileManagerDiskRecord {
    fn new(disk: storage_contract::DiskStatus, name: String) -> Self {
        let storage_contract::DiskStatus {
            slot_id,
            level,
            online,
            available_bytes,
            observation_failure,
            used_bytes,
            simulation,
            disk_id: _,
            upstream_slot_id: _,
            metadata_member: _,
            physical_device: _,
        } = disk;
        Self {
            name,
            slot_id,
            level,
            online,
            writable: level == 1,
            available_bytes,
            observation_failure,
            used_bytes,
            simulation,
        }
    }
}
impl FileManagerDisksResponse {
    pub(crate) fn from_snapshot(
        disks: Vec<storage_contract::DiskStatus>,
        topology: crate::TopologySnapshot,
    ) -> Result<Self, crate::ApiError> {
        let mut slots = topology
            .slots
            .into_iter()
            .map(|slot| (slot.slot_id.clone(), slot))
            .collect::<std::collections::BTreeMap<_, _>>();
        let changed = || {
            crate::ApiError::conflict(
                storage_contract::ErrorCode::ListingChanged,
                "disk membership changed while reading the directory listing",
            )
        };
        let mut records = Vec::with_capacity(disks.len());
        for disk in disks {
            let slot = slots.remove(&disk.slot_id).ok_or_else(changed)?;
            if slot.disk_id != disk.disk_id || slot.level != disk.level {
                return Err(changed());
            }
            records.push(FileManagerDiskRecord::new(
                disk,
                slot.directory_name()?.as_str().to_owned(),
            ));
        }
        if !slots.is_empty() {
            return Err(changed());
        }
        Ok(Self { disks: records })
    }
}
