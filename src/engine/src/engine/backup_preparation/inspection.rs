//! Read-only target inspection. These reports cannot activate a disk or restore
//! an Engine: native control evidence and the migration decision are separate.
use super::{FORMAT, Job, PhysicalBinding, TARGET_INTENT, snapshot};
use crate::{
    Error, ErrorCode, HistoryPackageReport, Result, canonical_json,
    disk::DiskRoot,
    history_package::{AdmittedState, preparation_checkpoint, verify_prepared_history_state},
    journal::decode_proof,
    object_hash,
};
use data_serialization::{Deserialize, Serialize};
use std::{path::Path, sync::atomic::AtomicBool};

#[derive(Serialize)]
struct HistoryCompletion<'a> {
    format: &'static str,
    job_hash: String,
    histories: &'a [HistoryPackageReport],
}

pub(super) fn history_marker(job: &Job, histories: &[HistoryPackageReport]) -> Result<Vec<u8>> {
    canonical_json(&HistoryCompletion {
        format: FORMAT,
        job_hash: object_hash("backup-preparation-job", job)?,
        histories,
    })
}

pub(super) const COMPLETE: &str = "snapshot-prepared.json";
const REPORT_FORMAT: &str = "zxs-backup-snapshot-prepared";

/// Material evidence only. Contains no control credentials or activation token.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BackupPreparationReport {
    pub format: String,
    pub job_hash: String,
    pub snapshot_hash: String,
    pub state_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    pub operations: usize,
    pub operation_principals: usize,
    pub histories: Vec<HistoryPackageReport>,
}

/// Independently verify only the held target's complete State and history
/// objects, including every retained body. Never falls back to the source pool,
/// repairs data, enrolls a disk, or treats this result as service authority.
/// Missing/corrupt/conflicting material and incomplete markers return errors.
pub fn offline_check_backup_preparation(path: &Path) -> Result<BackupPreparationReport> {
    let root = DiskRoot::open(path)?;
    let job: Job =
        decode_proof(&root.read_bytes(&path.join(TARGET_INTENT), crate::disk::MAX_METADATA)?)?;
    if job.format != FORMAT || job.request.target_paths.len() != 1 {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "unsupported backup preparation task",
        ));
    }
    if PhysicalBinding::of(&root)? != job.target {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "backup preparation root identity differs",
        ));
    }
    let stop = AtomicBool::new(false);
    let state = snapshot::read(&root, &job, &stop)?;
    let report = verify_material(&root, &job, &state, &stop)?;
    let history: json_codec::Value = storage_contract::strict_json(&root.read_bytes(
        &path.join("history-prepared.json"),
        crate::disk::MAX_METADATA,
    )?)?;
    if canonical_json(&history)? != history_marker(&job, &report.histories)? {
        return Err(Error::new(
            ErrorCode::HistoryPackageInvalid,
            "history completion belongs to different frozen material",
        ));
    }
    let completed: BackupPreparationReport =
        decode_proof(&root.read_bytes(&path.join(COMPLETE), crate::disk::MAX_METADATA)?)?;
    if canonical_json(&completed)? != canonical_json(&report)? {
        return Err(Error::new(
            ErrorCode::HistoryPackageInvalid,
            "snapshot completion differs from verified material",
        ));
    }
    if PhysicalBinding::of(&DiskRoot::open(path)?)? != job.target {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "backup preparation name changed during inspection",
        ));
    }
    Ok(report)
}

pub(super) fn verify_material(
    root: &DiskRoot,
    job: &Job,
    state: &AdmittedState,
    stop: &AtomicBool,
) -> Result<BackupPreparationReport> {
    if !state.state().histories.keys().eq(job.history_roots.keys()) {
        return Err(Error::new(
            ErrorCode::HistoryPackageInvalid,
            "task omits or adds histories outside the complete snapshot",
        ));
    }
    let mut reports = Vec::new();
    let mut files = 0u64;
    let mut bytes = 0u64;
    for history in state.state().histories.keys() {
        preparation_checkpoint(stop)?;
        let package = root.private_child(&root.path().join("histories").join(history))?;
        let report = verify_prepared_history_state(&package, state, history, &job.pool_id, stop)?;
        if report.manifest_hash != job.history_roots[history] {
            return Err(Error::new(
                ErrorCode::HistoryPackageInvalid,
                "history digest differs from frozen task",
            ));
        }
        files = files
            .checked_add(report.full_files as u64)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "prepared file count overflow"))?;
        bytes = bytes
            .checked_add(report.full_bytes)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "prepared byte count overflow"))?;
        reports.push(report);
    }
    if (files, bytes) != (job.total_files, job.total_bytes) {
        return Err(Error::new(
            ErrorCode::HistoryPackageInvalid,
            "prepared body closure differs from frozen task",
        ));
    }
    preparation_checkpoint(stop)?;
    Ok(BackupPreparationReport {
        format: REPORT_FORMAT.into(),
        job_hash: object_hash("backup-preparation-job", job)?,
        snapshot_hash: job.snapshot.clone(),
        state_hash: object_hash("backup-prepared-state", state.state())?,
        sequence: state.state().sequence,
        operations: state.state().operations.len(),
        operation_principals: state.state().operation_principals.len(),
        histories: reports,
    })
}
