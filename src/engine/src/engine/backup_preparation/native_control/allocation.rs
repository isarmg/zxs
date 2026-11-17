//! Lengths, not proofs: no forecast is written as a native target report.
use super::{
    BackupNativePreparationReport, BackupNativeTargetReport, BackupPreparationReport, COMPLETE,
    FORMAT, INTENT, MAX_TASK, NativeTask, RESERVE, TASK,
    archive::{BackupNativeControlReport, PreparedArchive, VisitOrder},
    completion_fanout::{Fanout, MaterialMode},
    invalid, named_target,
    runtime::Target,
};
use crate::disk::DiskRoot;
use crate::engine::backup_preparation::capacity::Census;
use crate::history_package::preparation_checkpoint;
use crate::{Error, ErrorCode, Result, canonical_json, object_hash};
use std::{path::Path, sync::atomic::AtomicBool};

pub(super) struct Admission {
    intent: Vec<u8>,
    // Once a marker exists, material is read/synced in place. No forecast or
    // new-material headroom check can prevent the exact original from retrying.
    completion_length: Option<usize>,
}

impl Admission {
    pub(super) fn capture(
        task: &NativeTask,
        original: &PreparedArchive,
        fanout: &Fanout,
        materials: Vec<BackupPreparationReport>,
    ) -> Result<Self> {
        let completion_length = match fanout.mode() {
            MaterialMode::CopyMissing => {
                Some(completion_length(task, &original.report, materials)?)
            }
            MaterialMode::ExistingOnly => None,
        };
        Ok(Self {
            intent: task.bytes()?,
            completion_length,
        })
    }

    pub(super) fn intent(&self) -> &[u8] {
        &self.intent
    }

    pub(super) fn require_new_system_intent(&self, system: &DiskRoot) -> Result<()> {
        let mut census = Census::new(system)?;
        if census.file_present(TASK.into(), self.intent.len() as u64)? {
            return Err(invalid("native SYSTEM intent appeared during admission"));
        }
        // An independent filesystem and one exact known intent. There was no
        // native-material reserve on SYSTEM; targets still keep theirs in full.
        census.require_with_reserve(0)
    }

    pub(super) fn require_material(
        &self,
        work: &[Target],
        task: &NativeTask,
        archive: &PreparedArchive,
        mode: MaterialMode,
        stop: &AtomicBool,
    ) -> Result<()> {
        if matches!(mode, MaterialMode::ExistingOnly) {
            return Ok(());
        }
        let completion = self
            .completion_length
            .ok_or_else(|| invalid("completed native target set cannot reenter copy admission"))?;
        for (target, job) in work.iter().zip(task.selection.jobs()) {
            preparation_checkpoint(stop)?;
            let root = &target.root;
            named_target(root, job)?;
            let mut census = Census::new(root)?;
            archive.visit_files(Path::new(""), VisitOrder::Address, |path, bytes| {
                preparation_checkpoint(stop)?;
                let absolute = root.path().join(&path);
                if census.file_present(path, bytes.len() as u64)? && root.read_bytes(&absolute, bytes.len())? != bytes {
                    return Err(Error::new(ErrorCode::ObjectCollision, "retained original archive differs from its frozen source; all copies preserved"));
                }
                Ok(())
            })?;
            if census.file_present(COMPLETE.into(), completion as u64)? {
                return Err(invalid(
                    "native completion appeared during material census; retry the exact original",
                ));
            }
            if census.file_present(INTENT.into(), self.intent.len() as u64)?
                && root.read_bytes(&root.path().join(INTENT), MAX_TASK)? != self.intent
            {
                return Err(invalid(
                    "retained native target intent belongs to another selection",
                ));
            }
            census.require_with_reserve(RESERVE)?;
            named_target(root, job)?;
        }
        Ok(())
    }
}

pub(super) fn completion_length(
    task: &NativeTask,
    control: &BackupNativeControlReport,
    materials: Vec<BackupPreparationReport>,
) -> Result<usize> {
    if materials.len() != task.selection.jobs().len() {
        return Err(invalid(
            "native completion must include every original target",
        ));
    }
    let empty = BackupNativePreparationReport {
        format: FORMAT.into(),
        task_hash: object_hash(FORMAT, task)?,
        targets: Vec::new(),
    };
    let mut length = canonical_json(&empty)?.len();
    for (index, material) in materials.into_iter().enumerate() {
        // The bounded, already-read history report is moved one at a time, not
        // cloned for every target. Only a length leaves this planning path.
        let entry = BackupNativeTargetReport {
            material,
            control: control.clone(),
        };
        let item = canonical_json(&entry)?.len();
        length = length
            .checked_add(usize::from(index != 0))
            .and_then(|n| n.checked_add(item))
            .filter(|length| *length <= MAX_TASK)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    "whole native completion exceeds its original reader budget",
                )
            })?;
    }
    Ok(length)
}
