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
    physical_device: String,
    #[serialization(with = "storage_contract::u64_string")]
    available_bytes: u64,
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
    slot_id: String,
    level: u32,
    online: bool,
    writable: bool,
    #[serialization(with = "storage_contract::u64_string")]
    available_bytes: u64,
    #[serialization(with = "storage_contract::u64_string")]
    used_bytes: u64,
    simulation: bool,
}
impl From<storage_contract::DiskStatus> for FileManagerDiskRecord {
    fn from(disk: storage_contract::DiskStatus) -> Self {
        let storage_contract::DiskStatus {
            slot_id,
            level,
            online,
            available_bytes,
            used_bytes,
            simulation,
            disk_id: _,
            upstream_slot_id: _,
            metadata_member: _,
            physical_device: _,
        } = disk;
        Self {
            slot_id,
            level,
            online,
            writable: level == 1,
            available_bytes,
            used_bytes,
            simulation,
        }
    }
}
impl From<Vec<storage_contract::DiskStatus>> for FileManagerDisksResponse {
    fn from(disks: Vec<storage_contract::DiskStatus>) -> Self {
        Self {
            disks: disks.into_iter().map(FileManagerDiskRecord::from).collect(),
        }
    }
}
