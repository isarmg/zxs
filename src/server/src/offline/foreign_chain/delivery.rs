//! New-intent admission only. A small NotStarted observation is not the size of
//! the final verified reply. Only success/error escapes; no predicted proof or
//! success flag can be published, installed or returned to a caller.
use super::*;
use crate::offline::service_worker::{encoded_fits, success_fits};
use storage_contract::offline_maintenance::Operation;

/// A directory alone is not an accepted intent. Bundle inspection rejects
/// unknown/colliding files; only the real matching plan can skip a *new*
/// feasibility forecast. An exact pending file keeps its ordinary retry path.
pub(super) fn has_exact_intent(
    host: &crate::offline::private_io::BundleDestination,
    name: &str,
    intent: &[u8],
) -> Result<bool> {
    let state = host.inspect(name, &[("plan.json", intent)])?;
    if state == platform_io::publication::BundleState::Absent {
        return Ok(false);
    }
    match host.read_operation(name, "plan.json", MAX_RECORD_BYTES)? {
        None => Ok(false),
        Some(bytes) if bytes == intent => Ok(true),
        Some(_) => Err("retained foreign intent differs from this exact original".into()),
    }
}

fn require(
    record: &impl Serialize,
    worker: &Output,
    operation: Operation,
    maintenance: impl FnOnce() -> Result<json_codec::Value>,
) -> Result<()> {
    if !encoded_fits(record, MAX_RECORD_BYTES)? {
        return Err("complete foreign result exceeds its existing durable reader limit; no new intent was saved".into());
    }
    if !success_fits::<WorkerRequest>(worker)? {
        return Err("complete foreign worker success exceeds its existing 64 KiB pipe limit; no new intent was saved".into());
    }
    if !encoded_fits(&maintenance()?, operation.response_limit())? {
        return Err("complete foreign maintenance result exceeds its existing response limit; no new intent was saved".into());
    }
    Ok(())
}

pub(super) fn material(plan: &Plan, mut observed: Report) -> Result<()> {
    observed.require_plan(plan)?;
    for target in &mut observed.targets {
        target.progress = Progress::PublishedCapsule;
    }
    let record = Completion {
        request_digest: digest(plan)?,
        report: observed.clone(),
    };
    let worker = Output::Report {
        report: observed.clone(),
    };
    require(&record, &worker, Operation::ForeignChainPrepare, || {
        preview_result(plan.clone(), Some(observed), true)
    })
}

pub(super) fn databases(
    input: &Original,
    plan: &Plan,
    expected: &databases::Expected,
) -> Result<()> {
    databases::require_targets(plan, expected.target_reports())?;
    let completed = databases::Observation {
        targets: expected.target_reports().to_vec(),
        statuses: expected
            .target_reports()
            .iter()
            .cloned()
            .map(|report| storage_engine::ForeignDatabaseStatus::Verified {
                report: Box::new(report),
            })
            .collect(),
    };
    let worker = Output::Databases {
        report: completed.clone(),
    };
    require(
        expected,
        &worker,
        Operation::ForeignChainDatabasesPrepare,
        || databases::response(input, expected.clone(), completed, true),
    )
}

pub(super) fn objects(
    input: &Original,
    plan: &Plan,
    expected: &storage_engine::ForeignIdentityClosureReport,
) -> Result<()> {
    let completed = objects::Observation {
        expected: expected.clone(),
        statuses: vec![objects::Progress::Verified; plan.binding.lower.len()],
    };
    let worker = Output::Objects {
        report: completed.clone(),
    };
    require(
        expected,
        &worker,
        Operation::ForeignChainObjectsPrepare,
        || objects::response(input, plan, completed, true),
    )
}
