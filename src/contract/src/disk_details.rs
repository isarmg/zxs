//! Administrator-facing disk observations and system-disk notes.
//! These fields are informational: neither hardware paths nor filesystem UUIDs
//! participate in ZXS managed-directory identity or storage admission.
use data_serialization::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum DiskMapping {
    Single,
    Multiple,
    Unknown,
}

impl<'de> Deserialize<'de> for DiskMapping {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "single" => Ok(Self::Single),
            "multiple" => Ok(Self::Multiple),
            "unknown" => Ok(Self::Unknown),
            _ => Err(data_serialization::de::Error::custom(
                "invalid disk mapping state",
            )),
        }
    }
}

fn nullable_text<'de, D: data_serialization::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::deserialize(deserializer)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiskDetailsRecord {
    pub slot_id: String,
    pub disk_id: String,
    /// The configured ZXS storage directory, not necessarily a mountpoint.
    pub root_path: String,
    #[serialization(deserialize_with = "nullable_text")]
    pub mount_path: Option<String>,
    #[serialization(deserialize_with = "nullable_text")]
    pub model: Option<String>,
    #[serialization(deserialize_with = "nullable_text")]
    pub serial: Option<String>,
    #[serialization(with = "crate::optional_u64_string")]
    pub capacity_bytes: Option<u64>,
    /// A current device node, never a stable identity.
    #[serialization(deserialize_with = "nullable_text")]
    pub device_path: Option<String>,
    /// Filesystem UUID only; not a namespace UUID or a ZXS disk ID.
    #[serialization(deserialize_with = "nullable_text")]
    pub filesystem_uuid: Option<String>,
    pub mapping: DiskMapping,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiskNoteRequest {
    pub expected_disk_id: String,
    pub expected_note: String,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiskNoteResponse {
    pub slot_id: String,
    pub disk_id: String,
    pub note: String,
}

#[cfg(test)]
#[path = "../../../test/contract/unit/disk_details.rs"]
mod tests;
