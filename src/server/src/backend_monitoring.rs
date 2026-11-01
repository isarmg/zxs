//! Non-authoritative observations for the private monitoring capability.
#[derive(Clone, Debug)]
pub struct DiskSample {
    pub slot_id: String,
    /// Internal diagnostic source; the telemetry boundary emits only stage and code.
    pub observation_failure: Option<std::sync::Arc<storage_engine::DiskObservationFailure>>,
    pub online: bool,
    pub read_only: bool,
    pub simulation: bool,
    pub total_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub used_bytes: Option<u64>,
    pub usage_percent: Option<f64>,
    pub power_watts: Option<f64>,
    pub power_unavailable_reason: Option<PowerUnavailable>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, data_serialization::Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum PowerUnavailable {
    SensorUnavailable,
    SensorUnreadable,
    Sampling,
    CounterReset,
    DiskOffline,
    SimulatedDisk,
    SensorAmbiguous,
}
impl<'de> data_serialization::Deserialize<'de> for PowerUnavailable {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let value = <String as data_serialization::Deserialize>::deserialize(deserializer)?;
        match value.as_str() {
            "sensor_unavailable" => Ok(Self::SensorUnavailable),
            "sensor_unreadable" => Ok(Self::SensorUnreadable),
            "sampling" => Ok(Self::Sampling),
            "counter_reset" => Ok(Self::CounterReset),
            "disk_offline" => Ok(Self::DiskOffline),
            "simulated_disk" => Ok(Self::SimulatedDisk),
            "sensor_ambiguous" => Ok(Self::SensorAmbiguous),
            _ => Err(data_serialization::de::Error::custom(
                "invalid power availability reason",
            )),
        }
    }
}

/// Safe telemetry classification, never paths or source messages.
#[derive(Clone, Debug, data_serialization::Serialize, data_serialization::Deserialize)]
#[serialization(
    tag = "stage",
    content = "code",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum DiskObservationIssue {
    Seal(storage_contract::ErrorCode),
    Capacity(storage_contract::ErrorCode),
}

impl From<&storage_engine::DiskObservationFailure> for DiskObservationIssue {
    fn from(error: &storage_engine::DiskObservationFailure) -> Self {
        match error.stage {
            storage_engine::DiskObservationStage::Seal => Self::Seal(error.source.code()),
            storage_engine::DiskObservationStage::Capacity => Self::Capacity(error.source.code()),
        }
    }
}
