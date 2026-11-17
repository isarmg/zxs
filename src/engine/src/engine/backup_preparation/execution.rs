//! One target's independent copy. Shared by the single-target and explicit
//! whole-unit batch workers; neither can publish a placement or authority.
use super::*;

pub(super) fn require_capacity(
    target: &DiskRoot,
    plans: &[PreparedHistoryPackage],
    snapshot: &snapshot::PreparedSnapshot,
    stop: &AtomicBool,
) -> Result<()> {
    let mut census = capacity::Census::new(target)?;
    let mut account = |path, length| {
        crate::history_package::preparation_checkpoint(stop)?;
        census.file(path, length)
    };
    for plan in plans {
        plan.visit_files(&mut account)?;
    }
    snapshot.visit_files(&mut account)?;
    census.require()
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
    require_capacity(target, &plans, snapshot, stop)?;
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
