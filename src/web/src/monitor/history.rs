mod wire;

use super::Metric;
use crate::api;
use std::collections::HashMap;

#[derive(Debug, error_definition::Error)]
pub(crate) enum Error {
    #[error("history response decode failed: {0}")]
    Decode(#[source] json_codec::Error),
    #[error("invalid UTC history timestamp: {0}")]
    Timestamp(#[source] date_time::error::Parse),
    #[error("invalid history field: {0}")]
    Field(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct History {
    pub from: f64,
    pub to: f64,
    pub interval: f64,
    samples: Vec<Sample>,
}
#[derive(Clone, Debug, PartialEq)]
struct Sample {
    time: f64,
    cpu: Option<f64>,
    cpu_power: Option<f64>,
    memory: Option<f64>,
    disks: HashMap<String, Disk>,
}
#[derive(Clone, Debug, PartialEq)]
struct Disk {
    observation_failure: Option<wire::DiskObservationIssue>,
    usage: Option<f64>,
    power: Option<f64>,
}
impl History {
    pub fn points(&self, metric: &Metric) -> Vec<(f64, Option<f64>)> {
        self.samples
            .iter()
            .map(|sample| {
                let value = match metric {
                    Metric::Cpu => sample.cpu,
                    Metric::CpuPower => sample.cpu_power,
                    Metric::Memory => sample.memory,
                    Metric::Disk { id, power, .. } => sample
                        .disks
                        .get(id)
                        .and_then(|disk| if *power { disk.power } else { disk.usage }),
                };
                (sample.time, value)
            })
            .collect()
    }
}

pub(super) fn decode(value: json_codec::Value, range: &str) -> Result<History, api::Error> {
    project(value, range).map_err(|source| {
        let mut error = api::error(
            storage_contract::ErrorCode::InvalidResponse,
            web_interface::i18n::t(
                "监测历史响应格式无效。",
                "Invalid monitoring history response.",
            ),
        );
        error.source = Some(std::sync::Arc::new(api::ErrorSource::MonitorHistory(
            source,
        )));
        error
    })
}
fn timestamp(value: &str) -> Result<f64, Error> {
    let parsed = date_time::OffsetDateTime::parse(
        value,
        &date_time::format_description::well_known::Rfc3339,
    )
    .map_err(Error::Timestamp)?;
    if !value.ends_with('Z') {
        return Err(Error::Field("UTC timestamp"));
    }
    Ok(parsed.unix_timestamp_nanos() as f64 / 1_000_000_000.)
}
fn ratio(used: Option<u64>, total: Option<u64>) -> Option<f64> {
    match (used, total) {
        (Some(used), Some(total)) if total > 0 && used <= total => {
            Some(used as f64 * 100. / total as f64)
        }
        _ => None,
    }
}
fn percent(value: Option<f64>) -> Result<Option<f64>, Error> {
    if value.is_some_and(|v| !v.is_finite() || !(0.0..=100.0).contains(&v)) {
        return Err(Error::Field("usage_percent"));
    }
    Ok(value)
}
fn power(value: Option<f64>) -> Result<Option<f64>, Error> {
    if value.is_some_and(|v| !v.is_finite() || v < 0.) {
        return Err(Error::Field("power_watts"));
    }
    Ok(value)
}
fn project(value: json_codec::Value, range: &str) -> Result<History, Error> {
    let response: wire::Response = json_codec::from_value(value).map_err(Error::Decode)?;
    if response.range != range || !super::RANGES.contains(&range) {
        return Err(Error::Field("range"));
    }
    let from = timestamp(&response.from)?;
    let to = timestamp(&response.to)?;
    if from >= to || response.interval_seconds <= 0 {
        return Err(Error::Field("history window"));
    }
    let samples = response
        .samples
        .into_iter()
        .map(project_sample)
        .collect::<Result<_, _>>()?;
    Ok(History {
        from,
        to,
        interval: response.interval_seconds as f64,
        samples,
    })
}
fn project_sample(sample: wire::Sample) -> Result<Sample, Error> {
    let mut disks = HashMap::with_capacity(sample.disks.len());
    for disk in sample.disks {
        let usage =
            percent(disk.usage_percent)?.or_else(|| ratio(disk.used_bytes, disk.total_bytes));
        let power = power(disk.power_watts)?;
        if disk.id.is_empty()
            || disks
                .insert(
                    disk.id,
                    Disk {
                        usage,
                        power,
                        observation_failure: disk.observation_failure,
                    },
                )
                .is_some()
        {
            return Err(Error::Field("duplicate or empty disk id"));
        }
    }
    Ok(Sample {
        time: timestamp(&sample.collected_at)?,
        cpu: percent(sample.cpu.usage_percent)?,
        cpu_power: power(sample.cpu.power_watts)?,
        memory: ratio(sample.memory.used_bytes, sample.memory.total_bytes),
        disks,
    })
}

#[cfg(test)]
#[path = "../../../../test/web/unit/monitor/history/tests.rs"]
mod tests;
