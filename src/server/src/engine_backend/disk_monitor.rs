//! Read-only sampling of an owned, detached disk observation plan.
use crate::DiskSample;
use storage_engine::{DiskObservation, DiskObservationPlan};

pub(super) fn observe(observation: DiskObservationPlan) -> Vec<DiskSample> {
    observation.sample().into_iter().map(disk_sample).collect()
}

fn disk_sample(observation: DiskObservation) -> DiskSample {
    let DiskObservation {
        status: disk,
        capacity,
    } = observation;
    let (power_watts, power_unavailable_reason) = crate::telemetry::disk_power(
        disk.physical_device.as_deref(),
        disk.simulation,
        disk.online,
    );
    let usage_percent = capacity
        .filter(|capacity| capacity.0 > 0)
        .map(|capacity| capacity.2 as f64 / capacity.0 as f64 * 100.0);
    DiskSample {
        slot_id: disk.slot_id,
        online: disk.online,
        read_only: disk.level != 1,
        simulation: disk.simulation,
        total_bytes: capacity.map(|c| c.0),
        available_bytes: capacity.map(|c| c.1),
        used_bytes: capacity.map(|c| c.2),
        usage_percent,
        power_watts,
        power_unavailable_reason,
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/disk_monitor_tests.rs"]
mod tests;
