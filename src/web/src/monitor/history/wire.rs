pub(super) use super::super::observations::DiskObservationIssue;
use data_serialization::{Deserialize, Deserializer};

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Response {
    pub range: String,
    pub interval_seconds: i64,
    pub from: String,
    pub to: String,
    pub samples: Vec<Sample>,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
#[expect(
    dead_code,
    reason = "Validate complete wire records before display projection"
)]
pub(super) struct Sample {
    pub collected_at: String,
    pub cpu: Cpu,
    pub memory: Memory,
    pub disks: Vec<Disk>,
    disk_sampling_unavailable: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
#[expect(
    dead_code,
    reason = "Validate complete wire records before display projection"
)]
pub(super) struct Cpu {
    #[serialization(deserialize_with = "nullable")]
    pub usage_percent: Option<f64>,
    logical_count: usize,
    #[serialization(deserialize_with = "nullable")]
    pub power_watts: Option<f64>,
    #[serialization(deserialize_with = "nullable")]
    power_unavailable_reason: Option<PowerUnavailable>,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
#[expect(
    dead_code,
    reason = "Validate complete wire records before display projection"
)]
pub(super) struct Memory {
    #[serialization(with = "storage_contract::optional_u64_string")]
    pub total_bytes: Option<u64>,
    #[serialization(with = "storage_contract::optional_u64_string")]
    pub used_bytes: Option<u64>,
    #[serialization(with = "storage_contract::optional_u64_string")]
    available_bytes: Option<u64>,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
#[expect(
    dead_code,
    reason = "Validate complete wire records before display projection"
)]
pub(super) struct Disk {
    #[serialization(deserialize_with = "nullable")]
    pub observation_failure: Option<DiskObservationIssue>,
    pub id: String,
    name: String,
    online: bool,
    read_only: bool,
    simulation: bool,
    #[serialization(with = "storage_contract::optional_u64_string")]
    pub total_bytes: Option<u64>,
    #[serialization(with = "storage_contract::optional_u64_string")]
    pub used_bytes: Option<u64>,
    #[serialization(with = "storage_contract::optional_u64_string")]
    available_bytes: Option<u64>,
    #[serialization(deserialize_with = "nullable")]
    pub usage_percent: Option<f64>,
    #[serialization(deserialize_with = "nullable")]
    pub power_watts: Option<f64>,
    #[serialization(deserialize_with = "nullable")]
    power_unavailable_reason: Option<PowerUnavailable>,
}
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}
#[derive(Deserialize)]
#[serialization(rename_all = "snake_case")]
enum PowerUnavailable {
    SensorUnavailable,
    SensorUnreadable,
    Sampling,
    CounterReset,
    DiskOffline,
    SimulatedDisk,
    SensorAmbiguous,
}
