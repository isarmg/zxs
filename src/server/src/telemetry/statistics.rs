//! Sufficient per-metric statistics survive nulls and repeated aggregation.
use super::Snapshot;
use data_serialization::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Statistics {
    // Each metric has its own valid observation count; an absent sensor never
    // contributes to another metric's denominator. Integer sums stay exact.
    floating: BTreeMap<String, (f64, u32)>,
    bytes: BTreeMap<String, (u128, u32)>,
}

impl Statistics {
    pub(super) fn observation(status: &Snapshot) -> Self {
        let mut result = Self::default();
        result.float("cpu/usage", status.cpu.usage_percent.map(f64::from));
        result.float("cpu/power", status.cpu.power_watts);
        result.byte("memory/total", status.memory.total_bytes);
        result.byte("memory/used", status.memory.used_bytes);
        result.byte("memory/available", status.memory.available_bytes);
        for disk in &status.disks {
            result.float(&format!("disk/{}/usage", disk.id), disk.usage_percent);
            result.float(&format!("disk/{}/power", disk.id), disk.power_watts);
            result.byte(&format!("disk/{}/total", disk.id), disk.total_bytes);
            result.byte(&format!("disk/{}/used", disk.id), disk.used_bytes);
            result.byte(&format!("disk/{}/available", disk.id), disk.available_bytes);
        }
        result
    }

    fn float(&mut self, key: &str, value: Option<f64>) {
        self.floating
            .insert(key.into(), value.map_or((0.0, 0), |value| (value, 1)));
    }
    fn byte(&mut self, key: &str, value: Option<u64>) {
        self.bytes.insert(
            key.into(),
            value.map_or((0, 0), |value| (u128::from(value), 1)),
        );
    }
    pub(super) fn merge(&mut self, source: &Self) {
        for (key, (sum, count)) in &source.floating {
            let current = self.floating.entry(key.clone()).or_default();
            current.0 += sum;
            current.1 += count;
        }
        for (key, (sum, count)) in &source.bytes {
            let current = self.bytes.entry(key.clone()).or_default();
            current.0 += sum;
            current.1 += count;
        }
    }
    fn floating_mean(&self, key: &str) -> Option<f64> {
        let (sum, count) = self.floating.get(key)?;
        (*count > 0).then(|| *sum / f64::from(*count))
    }
    fn byte_mean(&self, key: &str) -> Option<u64> {
        let (sum, count) = self.bytes.get(key)?;
        (*count > 0).then(|| {
            u64::try_from(*sum / u128::from(*count))
                .expect("invariant: admitted byte statistics have a u64 average")
        })
    }
    pub(super) fn apply(&self, status: &mut Snapshot) {
        status.cpu.usage_percent = self.floating_mean("cpu/usage").map(|value| value as f32);
        status.cpu.power_watts = self.floating_mean("cpu/power");
        if status.cpu.power_watts.is_some() {
            status.cpu.power_unavailable_reason = None;
        }
        status.memory.total_bytes = self.byte_mean("memory/total");
        status.memory.used_bytes = self.byte_mean("memory/used");
        status.memory.available_bytes = self.byte_mean("memory/available");
        for disk in &mut status.disks {
            disk.usage_percent = self.floating_mean(&format!("disk/{}/usage", disk.id));
            disk.power_watts = self.floating_mean(&format!("disk/{}/power", disk.id));
            if disk.power_watts.is_some() {
                disk.power_unavailable_reason = None;
            }
            disk.total_bytes = self.byte_mean(&format!("disk/{}/total", disk.id));
            disk.used_bytes = self.byte_mean(&format!("disk/{}/used", disk.id));
            disk.available_bytes = self.byte_mean(&format!("disk/{}/available", disk.id));
        }
    }

    pub(super) fn valid_for(&self, status: &Snapshot, observations: u32) -> bool {
        let observed = Self::observation(status);
        if self.floating.keys().ne(observed.floating.keys())
            || self.bytes.keys().ne(observed.bytes.keys())
        {
            return false;
        }
        self.floating.iter().all(|(key, (sum, count))| {
            let (displayed, present) = observed.floating[key];
            *count <= observations
                && sum.is_finite()
                && *sum >= 0.0
                && ((*count == 0 && *sum == 0.0 && present == 0)
                    || (*count > 0
                        && present == 1
                        && (*sum / f64::from(*count) - displayed).abs()
                            <= displayed.abs().max(1.0) * 0.000_001))
        }) && self.bytes.iter().all(|(key, (sum, count))| {
            let (displayed, present) = observed.bytes[key];
            *count <= observations
                && *sum <= u128::from(u64::MAX) * u128::from(*count)
                && ((*count == 0 && *sum == 0 && present == 0)
                    || (*count > 0 && present == 1 && *sum / u128::from(*count) == displayed))
        })
    }
}
