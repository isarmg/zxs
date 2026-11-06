//! File-manager disk records, separate from topology and monitor observations.
use data_serialization::{Deserialize, Deserializer, de::Error as _};
use json_codec::Value;
use std::num::NonZeroU32;
use storage_contract::DiskObservationIssue;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Disk {
    pub name: String,
    pub slot_id: String,
    pub level: NonZeroU32,
    pub online: bool,
    /// Directory mode reported by the server, not current write authority.
    pub writable: bool,
    pub available_bytes: Option<u64>,
    pub observation_failure: Option<DiskObservationIssue>,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct DiskList {
    disks: Vec<DiskRecord>,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct DiskRecord {
    name: String,
    slot_id: String,
    level: u32,
    online: bool,
    writable: bool,
    #[serialization(with = "storage_contract::optional_u64_string")]
    available_bytes: Option<u64>,
    #[serialization(with = "storage_contract::u64_string", rename = "used_bytes")]
    _used_bytes: u64,
    #[serialization(rename = "simulation")]
    _simulation: bool,
    #[serialization(deserialize_with = "nullable_issue")]
    observation_failure: Option<DiskObservationIssue>,
}

fn nullable_issue<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<DiskObservationIssue>, D::Error> {
    Option::deserialize(deserializer)
}

pub(super) fn parse(value: Value) -> Result<Vec<Disk>, json_codec::Error> {
    DiskList::deserialize(value)?
        .disks
        .into_iter()
        .map(|disk| {
            if disk.slot_id.is_empty() {
                return Err(json_codec::Error::custom("empty disk slot id"));
            }
            name_policy::validate_name(&disk.name).map_err(json_codec::Error::custom)?;
            let level = NonZeroU32::new(disk.level)
                .ok_or_else(|| json_codec::Error::custom("disk level must be positive"))?;
            if level.get() != 1 && disk.writable {
                return Err(json_codec::Error::custom("backup disk cannot be writable"));
            }
            if disk.online
                && matches!(
                    disk.observation_failure,
                    Some(DiskObservationIssue::Seal(_))
                )
            {
                return Err(json_codec::Error::custom(
                    "online disk cannot have a failed identity observation",
                ));
            }
            Ok(Disk {
                name: disk.name,
                slot_id: disk.slot_id,
                level,
                online: disk.online,
                writable: disk.writable,
                available_bytes: disk.available_bytes,
                observation_failure: disk.observation_failure,
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/model/disks_tests.rs"]
mod tests;
