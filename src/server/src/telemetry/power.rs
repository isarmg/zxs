//! Read-only Linux sensor support. Never infer power from utilization or TDP.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use crate::PowerUnavailable;
#[derive(Clone, Debug, Default)]
pub(super) struct PowerReading {
    pub watts: Option<f64>,
    pub reason: Option<PowerUnavailable>,
}
impl PowerReading {
    pub(super) fn unavailable(reason: PowerUnavailable) -> Self {
        Self {
            watts: None,
            reason: Some(reason),
        }
    }
    fn measured(watts: f64) -> Self {
        Self {
            watts: Some(watts),
            reason: None,
        }
    }
}

#[derive(Clone)]
struct Energy {
    microjoules: u64,
    range: Option<u64>,
}

pub(super) struct CpuPowerSampler {
    root: PathBuf,
    previous: Option<(Instant, BTreeMap<PathBuf, Energy>)>,
}
impl CpuPowerSampler {
    pub(super) fn new() -> Self {
        Self {
            root: PathBuf::from("/sys/class/powercap"),
            previous: None,
        }
    }

    pub(super) fn sample(&mut self) -> PowerReading {
        self.sample_at(Instant::now())
    }

    fn sample_at(&mut self, now: Instant) -> PowerReading {
        let mut zones = BTreeSet::new();
        discover_packages(&self.root, 0, &mut zones, &mut BTreeSet::new());
        if zones.is_empty() {
            self.previous = None;
            return PowerReading::unavailable(PowerUnavailable::SensorUnavailable);
        }
        let mut current = BTreeMap::new();
        for zone in zones {
            let microjoules = match read_u64(&zone.join("energy_uj")) {
                Ok(value) => value,
                Err(error) => {
                    tracing::debug!(error = ?error, "CPU energy sensor unreadable");
                    self.previous = None;
                    return PowerReading::unavailable(PowerUnavailable::SensorUnreadable);
                }
            };
            current.insert(
                zone.clone(),
                Energy {
                    microjoules,
                    range: match read_u64(&zone.join("max_energy_range_uj")) {
                        Ok(value) => (value > 0).then_some(value),
                        Err(error) => {
                            tracing::debug!(error = ?error, "CPU energy counter wrap range unavailable");
                            None
                        }
                    },
                },
            );
        }
        let previous = self.previous.replace((now, current.clone()));
        let Some((then, previous)) = previous else {
            return PowerReading::unavailable(PowerUnavailable::Sampling);
        };
        let seconds = now.saturating_duration_since(then).as_secs_f64();
        if seconds <= 0.0 || previous.keys().ne(current.keys()) {
            return PowerReading::unavailable(PowerUnavailable::Sampling);
        }
        let mut energy = 0_u128;
        for (path, next) in &current {
            let old = &previous[path];
            let Some(delta) = energy_delta(old, next) else {
                return PowerReading::unavailable(PowerUnavailable::CounterReset);
            };
            energy += u128::from(delta);
        }
        PowerReading::measured(energy as f64 / 1_000_000.0 / seconds)
    }
}

fn energy_delta(previous: &Energy, current: &Energy) -> Option<u64> {
    if current.range != previous.range {
        return None;
    }
    if current.microjoules >= previous.microjoules {
        return Some(current.microjoules - previous.microjoules);
    }
    let range = current.range?;
    if previous.microjoules > range || current.microjoules > range {
        return None;
    }
    (range - previous.microjoules).checked_add(current.microjoules)
}

// Optional sysfs probes may disappear between samples; keep the cause observable.
fn sensor_value<T>(result: std::io::Result<T>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::debug!(error = ?error, "optional power sensor unavailable");
            None
        }
    }
}

fn discover_packages(
    root: &Path,
    depth: usize,
    result: &mut BTreeSet<PathBuf>,
    seen: &mut BTreeSet<PathBuf>,
) {
    if depth > 4 || seen.len() >= 256 {
        return;
    }
    let Some(canonical) = sensor_value(root.canonicalize()) else {
        return;
    };
    if !seen.insert(canonical.clone()) {
        return;
    }
    if sensor_value(fs::read_to_string(canonical.join("name"))).is_some_and(|name| {
        name.trim()
            .strip_prefix("package-")
            .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_digit()))
    }) {
        result.insert(canonical);
        // A package already contains its core/uncore domains: do not add them twice.
        return;
    }
    let Some(entries) = sensor_value(fs::read_dir(root)) else {
        return;
    };
    for entry in entries.take(256) {
        let Some(entry) = sensor_value(entry) else {
            continue;
        };
        if entry.path().is_dir() {
            discover_packages(&entry.path(), depth + 1, result, seen);
        }
    }
}

fn read_u64(path: &Path) -> std::io::Result<u64> {
    fs::read_to_string(path)?
        .trim()
        .parse()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

/// The storage engine provides a previously verified physical sysfs identity.
/// A sensor must belong to that drive (or its own SCSI device), not merely to
/// the same PCI controller or motherboard. Several power rails are ambiguous;
/// without a documented total channel we deliberately report no measurement.
#[cfg(any(feature = "core", test))]
pub(crate) fn disk_power(
    physical_device: &str,
    simulation: bool,
    online: bool,
) -> (Option<f64>, Option<PowerUnavailable>) {
    let reading = if !online {
        PowerReading::unavailable(PowerUnavailable::DiskOffline)
    } else if simulation {
        PowerReading::unavailable(PowerUnavailable::SimulatedDisk)
    } else {
        read_disk_power(
            Path::new(physical_device),
            Path::new("/sys/class/hwmon"),
            Some(Path::new("/sys/devices")),
        )
    };
    (reading.watts, reading.reason)
}

#[cfg(any(feature = "core", test))]
fn read_disk_power(
    physical_device: &Path,
    hwmon_root: &Path,
    allowed_root: Option<&Path>,
) -> PowerReading {
    let Some(physical) = sensor_value(physical_device.canonicalize()) else {
        return PowerReading::unavailable(PowerUnavailable::SensorUnavailable);
    };
    if allowed_root.is_some_and(|root| !physical.starts_with(root)) {
        return PowerReading::unavailable(PowerUnavailable::SensorUnavailable);
    }
    let mut owners = vec![physical.clone()];
    if let Some(device) = sensor_value(physical.join("device").canonicalize()) {
        owners.push(device);
    }
    let Some(entries) = sensor_value(fs::read_dir(hwmon_root)) else {
        return PowerReading::unavailable(PowerUnavailable::SensorUnavailable);
    };
    let mut channels = BTreeMap::<(PathBuf, String), PathBuf>::new();
    for entry in entries.take(256) {
        let Some(entry) = sensor_value(entry) else {
            continue;
        };
        let path = entry.path();
        let owner = sensor_value(path.join("device").canonicalize()).or_else(|| {
            let canonical = sensor_value(path.canonicalize())?;
            canonical.parent()?.parent().map(Path::to_path_buf)
        });
        let Some(owner) = owner else {
            continue;
        };
        if !owners
            .iter()
            .any(|drive| owner == *drive || owner.starts_with(drive))
        {
            continue;
        }
        let Some(files) = sensor_value(fs::read_dir(&path)) else {
            continue;
        };
        for file in files.take(512) {
            let Some(file) = sensor_value(file) else {
                continue;
            };
            let name = file.file_name().to_string_lossy().into_owned();
            let Some((stem, kind)) = name.rsplit_once('_') else {
                continue;
            };
            if !matches!(kind, "input" | "average")
                || !stem
                    .strip_prefix("power")
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            {
                continue;
            }
            let Some(sensor) = sensor_value(path.canonicalize()) else {
                continue;
            };
            let key = (sensor, stem.to_string());
            let candidate = file.path();
            if kind == "input" || !channels.contains_key(&key) {
                channels.insert(key, candidate);
            }
        }
    }
    if channels.len() > 1 {
        return PowerReading::unavailable(PowerUnavailable::SensorAmbiguous);
    }
    let Some(path) = channels.into_values().next() else {
        return PowerReading::unavailable(PowerUnavailable::SensorUnavailable);
    };
    match read_u64(&path) {
        Ok(microwatts) => PowerReading::measured(microwatts as f64 / 1_000_000.0),
        Err(error) => {
            tracing::debug!(error = ?error, "disk power sensor unreadable");
            PowerReading::unavailable(PowerUnavailable::SensorUnreadable)
        }
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/telemetry/power/tests.rs"]
mod tests;
