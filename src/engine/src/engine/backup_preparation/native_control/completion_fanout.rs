//! A stage's first whole-set marker is written only after *all* material was
//! verified. Interrupted marker fan-out may not turn a peer back into a copy target.
use super::{Job, Result, invalid, named_target, runtime::Target};
use crate::disk::DiskRoot;
use crate::history_package::{
    install_preparation_bytes, preparation_checkpoint, preparation_path_exists,
    sync_existing_preparation_bytes,
};
use std::{path::Path, sync::atomic::AtomicBool};

#[derive(Clone, Copy)]
pub(super) enum MaterialMode {
    CopyMissing,
    ExistingOnly,
}
impl MaterialMode {
    pub(super) fn write(self, root: &DiskRoot, path: &Path, bytes: &[u8]) -> Result<()> {
        match self {
            Self::CopyMissing => install_preparation_bytes(root, path, bytes),
            Self::ExistingOnly => sync_existing_preparation_bytes(root, path, bytes),
        }
    }
}

pub(super) struct Fanout {
    marker: &'static str,
    seen: Vec<bool>,
}
impl Fanout {
    pub(super) fn observe(marker: &'static str, targets: &[Target], jobs: &[Job]) -> Result<Self> {
        if targets.is_empty() || targets.len() != jobs.len() {
            return Err(invalid(
                "incomplete original target set for Backup marker fan-out",
            ));
        }
        let mut result = Self {
            marker,
            seen: vec![false; targets.len()],
        };
        result.refresh(targets, jobs)?;
        Ok(result)
    }

    pub(super) fn refresh(&mut self, targets: &[Target], jobs: &[Job]) -> Result<()> {
        if targets.len() != self.seen.len() || jobs.len() != self.seen.len() {
            return Err(invalid("Backup marker fan-out target set changed"));
        }
        for ((target, job), seen) in targets.iter().zip(jobs).zip(&mut self.seen) {
            named_target(&target.root, job)?;
            let present =
                preparation_path_exists(&target.root, &target.root.path().join(self.marker))?;
            if *seen && !present {
                return Err(invalid(
                    "previously observed Backup completion is missing; original set retained",
                ));
            }
            *seen = present;
        }
        Ok(())
    }

    pub(super) fn mode(&self) -> MaterialMode {
        if self.seen.iter().any(|present| *present) {
            MaterialMode::ExistingOnly
        } else {
            MaterialMode::CopyMissing
        }
    }

    /// Called only with bytes derived from fresh physical readback of every
    /// original material. Reject a conflict even on the last peer before adding
    /// the first missing marker. Previously observed markers are never recreated.
    pub(super) fn finish(
        &mut self,
        targets: &[Target],
        jobs: &[Job],
        expected: &[u8],
        maximum: usize,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.refresh(targets, jobs)?;
        for (target, &present) in targets.iter().zip(&self.seen) {
            preparation_checkpoint(stop)?;
            if present
                && target
                    .root
                    .read_bytes(&target.root.path().join(self.marker), maximum)?
                    != expected
            {
                return Err(invalid(
                    "retained Backup completion differs from the verified whole set",
                ));
            }
        }
        for ((target, job), &present) in targets.iter().zip(jobs).zip(&self.seen) {
            preparation_checkpoint(stop)?;
            named_target(&target.root, job)?;
            let path = target.root.path().join(self.marker);
            if present {
                sync_existing_preparation_bytes(&target.root, &path, expected)?;
            } else {
                install_preparation_bytes(&target.root, &path, expected)?;
            }
        }
        for (target, job) in targets.iter().zip(jobs) {
            preparation_checkpoint(stop)?;
            if target
                .root
                .read_bytes(&target.root.path().join(self.marker), maximum)?
                != expected
            {
                return Err(invalid(
                    "Backup completion readback differs from the verified whole set",
                ));
            }
            named_target(&target.root, job)?;
        }
        Ok(())
    }
}
