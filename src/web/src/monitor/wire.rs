//! Exact current telemetry response; the server crate cannot be linked into WASM.
use data_serialization::{Deserialize, Deserializer};
pub(super) use storage_contract::DiskObservationIssue;

#[derive(Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub(super) struct Response {
    pub range: String,
    pub interval_seconds: i64,
    pub from: String,
    pub to: String,
    pub samples: Vec<Sample>,
}
#[derive(Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub(super) struct Sample {
    pub collected_at: String,
    pub cpu: Cpu,
    pub memory: Memory,
    pub disks: Vec<Disk>,
    pub disk_sampling_unavailable: bool,
}
#[derive(Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub(super) struct Cpu {
    #[serialization(deserialize_with = "percent")]
    pub usage_percent: Option<f64>,
    pub logical_count: usize,
    #[serialization(deserialize_with = "power")]
    pub power_watts: Option<f64>,
    #[serialization(deserialize_with = "nullable")]
    pub power_unavailable_reason: Option<PowerUnavailable>,
}
#[derive(Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub(super) struct Memory {
    #[serialization(with = "storage_contract::optional_u64_string")]
    pub total_bytes: Option<u64>,
    #[serialization(with = "storage_contract::optional_u64_string")]
    pub used_bytes: Option<u64>,
    #[serialization(with = "storage_contract::optional_u64_string")]
    available_bytes: Option<u64>,
}
#[derive(Deserialize, PartialEq)]
#[serialization(deny_unknown_fields)]
pub(super) struct Disk {
    #[serialization(deserialize_with = "nullable")]
    pub observation_failure: Option<DiskObservationIssue>,
    pub id: String,
    pub name: String,
    pub online: bool,
    read_only: bool,
    simulation: bool,
    #[serialization(with = "storage_contract::optional_u64_string")]
    pub total_bytes: Option<u64>,
    #[serialization(with = "storage_contract::optional_u64_string")]
    pub used_bytes: Option<u64>,
    #[serialization(with = "storage_contract::optional_u64_string")]
    available_bytes: Option<u64>,
    #[serialization(deserialize_with = "percent")]
    pub usage_percent: Option<f64>,
    #[serialization(deserialize_with = "power")]
    pub power_watts: Option<f64>,
    #[serialization(deserialize_with = "nullable")]
    pub power_unavailable_reason: Option<PowerUnavailable>,
}
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}
#[derive(Clone, Copy, PartialEq)]
pub(super) enum PowerUnavailable {
    SensorUnavailable,
    SensorUnreadable,
    Sampling,
    CounterReset,
    DiskOffline,
    SimulatedDisk,
    SensorAmbiguous,
}
impl<'de> Deserialize<'de> for PowerUnavailable {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = String::deserialize(d)?;
        match value.as_str() {
            "sensor_unavailable" => Ok(Self::SensorUnavailable),
            "sensor_unreadable" => Ok(Self::SensorUnreadable),
            "sampling" => Ok(Self::Sampling),
            "counter_reset" => Ok(Self::CounterReset),
            "disk_offline" => Ok(Self::DiskOffline),
            "simulated_disk" => Ok(Self::SimulatedDisk),
            "sensor_ambiguous" => Ok(Self::SensorAmbiguous),
            unknown => Err(data_serialization::de::Error::unknown_variant(
                unknown,
                &[
                    "sensor_unavailable",
                    "sensor_unreadable",
                    "sampling",
                    "counter_reset",
                    "disk_offline",
                    "simulated_disk",
                    "sensor_ambiguous",
                ],
            )),
        }
    }
}

fn percent<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    let value = Option::<f64>::deserialize(d)?;
    if value.is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value)) {
        return Err(data_serialization::de::Error::custom(
            "invalid usage_percent",
        ));
    }
    Ok(value)
}
fn power<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    let value = Option::<f64>::deserialize(d)?;
    if value.is_some_and(|value| !value.is_finite() || value < 0.) {
        return Err(data_serialization::de::Error::custom("invalid power_watts"));
    }
    Ok(value)
}
