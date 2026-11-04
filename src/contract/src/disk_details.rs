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
    pub health: DiskHealth,
    pub note: String,
}

/// A device-reported observation, never a guarantee of physical health or replicas.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiskHealth {
    Unavailable {
        reason: DiskHealthUnavailable,
    },
    Smart {
        #[serialization(with = "crate::u64_string")]
        observed_at_unix_ms: u64,
        /// Whole device queried; may differ from the mounted partition path.
        device_path: String,
        assessment: DiskHealthAssessment,
        #[serialization(deserialize_with = "nullable_temperature")]
        temperature_celsius: Option<i32>,
        #[serialization(with = "crate::optional_u64_string")]
        power_on_hours: Option<u64>,
    },
}

fn nullable_temperature<'de, D: data_serialization::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i32>, D::Error> {
    Option::deserialize(deserializer)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum DiskHealthAssessment {
    Passed,
    Warning,
    Failed,
}

impl<'de> Deserialize<'de> for DiskHealthAssessment {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "passed" => Ok(Self::Passed),
            "warning" => Ok(Self::Warning),
            "failed" => Ok(Self::Failed),
            _ => Err(data_serialization::de::Error::custom(
                "invalid disk health state",
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum DiskHealthUnavailable {
    MappingUnknown,
    MultipleDevices,
    UnsupportedDevice,
    ToolMissing,
    PermissionDenied,
    DeviceChanged,
    Busy,
    TimedOut,
    LowPower,
    QueryFailed,
    InvalidOutput,
    IncompleteOutput,
}

impl<'de> Deserialize<'de> for DiskHealthUnavailable {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "mapping_unknown" => Ok(Self::MappingUnknown),
            "multiple_devices" => Ok(Self::MultipleDevices),
            "unsupported_device" => Ok(Self::UnsupportedDevice),
            "tool_missing" => Ok(Self::ToolMissing),
            "permission_denied" => Ok(Self::PermissionDenied),
            "device_changed" => Ok(Self::DeviceChanged),
            "busy" => Ok(Self::Busy),
            "timed_out" => Ok(Self::TimedOut),
            "low_power" => Ok(Self::LowPower),
            "query_failed" => Ok(Self::QueryFailed),
            "invalid_output" => Ok(Self::InvalidOutput),
            "incomplete_output" => Ok(Self::IncompleteOutput),
            _ => Err(data_serialization::de::Error::custom(
                "invalid disk health state",
            )),
        }
    }
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
