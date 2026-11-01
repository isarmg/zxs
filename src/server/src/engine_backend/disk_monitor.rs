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
    let (capacity, observation_failure) = match capacity {
        Ok(capacity) => (Some(capacity), None),
        Err(error) => {
            let issue = observation_issue(&error);
            (
                None,
                Some(std::sync::Arc::new(crate::DiskObservationFailure::new(
                    issue, error,
                ))),
            )
        }
    };
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
        observation_failure,
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

fn observation_issue(
    error: &storage_engine::DiskObservationFailure,
) -> crate::DiskObservationIssue {
    match error.stage {
        storage_engine::DiskObservationStage::Seal => {
            crate::DiskObservationIssue::Seal(error.source.code())
        }
        storage_engine::DiskObservationStage::Capacity => {
            crate::DiskObservationIssue::Capacity(error.source.code())
        }
    }
}

/// Public listing projection: offline members remain rows, with unknown capacity.
/// The concrete cause reaches this application boundary, not the wire response.
pub(super) fn diagnostics(plan: DiskObservationPlan) -> Vec<storage_contract::DiskStatus> {
    plan.sample().into_iter().map(diagnostic_disk).collect()
}

fn diagnostic_disk(observation: DiskObservation) -> storage_contract::DiskStatus {
    let disk = observation.status;
    let (available_bytes, observation_failure) = match observation.capacity {
        Ok(capacity) => (Some(capacity.1), None),
        Err(error) => {
            tracing::debug!(slot_id = %error.slot_id, stage = ?error.stage,
                error = ?error, "disk listing observation unavailable");
            (None, Some(observation_issue(&error)))
        }
    };
    storage_contract::DiskStatus {
        slot_id: disk.slot_id,
        disk_id: disk.disk_id,
        level: disk.level,
        upstream_slot_id: disk.upstream_slot_id,
        online: disk.online,
        metadata_member: disk.metadata_member,
        physical_device: disk.physical_device,
        available_bytes,
        observation_failure,
        used_bytes: disk.used_bytes,
        simulation: disk.simulation,
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/engine_backend/disk_monitor_tests.rs"]
mod tests;
