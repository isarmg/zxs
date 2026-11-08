//! One target's independent copy. Shared by the single-target and explicit
//! whole-unit batch workers; neither can publish a placement or authority.
use super::*;

pub(super) fn require_capacity(
    target: &DiskRoot,
    plans: &[PreparedHistoryPackage],
    snapshot: &snapshot::PreparedSnapshot,
) -> Result<()> {
    let remaining = plans
        .iter()
        .try_fold(snapshot.remaining_bytes(target)?, |total, plan| {
            total
                .checked_add(plan.remaining_bytes(target)?)
                .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "backup space overflow"))
        })?;
    let required = remaining
        .checked_add(if remaining == 0 { 64 * 1024 } else { RESERVE })
        .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "backup space overflow"))?;
    if target.available()? < required {
        return Err(Error::new(
            ErrorCode::NoSpace,
            "backup target cannot hold the complete frozen unit",
        ));
    }
    Ok(())
}

pub(super) fn copy_target(
    job: &Job,
    target: &DiskRoot,
    plans: Vec<PreparedHistoryPackage>,
    snapshot: &snapshot::PreparedSnapshot,
    stop: &AtomicBool,
    update: &impl Fn(&dyn Fn(&mut BackupPreparationStatus)) -> Result<()>,
) -> Result<BackupPreparationReport> {
    named_target(target, job)?;
    crate::history_package::preparation_checkpoint(stop)?;
    require_capacity(target, &plans, snapshot)?;
    update(&|_| {})?;
    snapshot.copy(target, stop)?;
    let admitted = snapshot::read(target, job, stop)?;
    let mut reports = Vec::new();
    for plan in plans {
        named_target(target, job)?;
        update(&|status| status.phase = BackupPreparationPhase::Copying)?;
        let path = target.path().join("histories").join(plan.history_id());
        target.private_dir(&path)?;
        let package = target.private_child(&path)?;
        let report = plan.copy(
            &package,
            stop,
            |bytes| {
                update(&|status| {
                    status.completed_files += 1;
                    status.completed_bytes += bytes;
                })
            },
            || update(&|status| status.phase = BackupPreparationPhase::Verifying),
        )?;
        reports.push(report);
    }
    update(&|status| status.phase = BackupPreparationPhase::Verifying)?;
    named_target(target, job)?;
    crate::history_package::preparation_checkpoint(stop)?;
    crate::history_package::install_preparation_bytes(
        target,
        &target.path().join("history-prepared.json"),
        &inspection::history_marker(job, &reports)?,
    )?;
    let report = inspection::verify_material(target, job, &admitted, stop)?;
    let bytes = canonical_json(&report)?;
    if bytes.len() > MAX_METADATA {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "snapshot completion exceeds metadata budget",
        ));
    }
    crate::history_package::install_preparation_bytes(
        target,
        &target.path().join(inspection::COMPLETE),
        &bytes,
    )?;
    named_target(target, job)?;
    Ok(report)
}
