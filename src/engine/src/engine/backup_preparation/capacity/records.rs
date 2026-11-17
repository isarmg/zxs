//! Sizes only. A forecast cannot produce report bytes, a publication or a
//! target verification. Actual completions are still encoded after local readback.
use crate::disk::MAX_METADATA;
use crate::engine::backup_preparation::{
    Job, TARGET_INTENT, batch,
    capacity::{CapacityError, Census, EntryCost},
    inspection,
};
use crate::history_package::{PreparedHistoryPackage, preparation_checkpoint};
use crate::{Result, canonical_json};
use platform_io::fs_safety::EntryName;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy)]
pub(in crate::engine::backup_preparation) struct Records<'a> {
    job: &'a Job,
    batch: Option<batch::BatchEnvelope>,
}

impl<'a> Records<'a> {
    pub(in crate::engine::backup_preparation) fn single(job: &'a Job) -> Self {
        Self { job, batch: None }
    }

    pub(in crate::engine::backup_preparation) fn batch(
        job: &'a Job,
        batch: batch::BatchEnvelope,
    ) -> Self {
        Self {
            job,
            batch: Some(batch),
        }
    }

    pub(in crate::engine::backup_preparation) fn job(self) -> &'a Job {
        self.job
    }

    pub(super) fn charge(
        self,
        census: &mut Census<'_>,
        plans: &[PreparedHistoryPackage],
        counts: inspection::ReportCounts,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.visit(plans, counts, stop, |name, bytes| {
            // All control names are root leaves disjoint from histories and
            // pool-snapshot. They have no implicit parent or shared path cursor.
            census.entry(&name.as_relative(), true, EntryCost::File(bytes as u64))?;
            Ok(())
        })
    }

    fn visit(
        self,
        plans: &[PreparedHistoryPackage],
        counts: inspection::ReportCounts,
        stop: &AtomicBool,
        mut account: impl FnMut(EntryName, usize) -> Result<()>,
    ) -> Result<()> {
        let (history_base, report_base) = inspection::empty_envelope_lengths(self.job, counts)?;
        let mut items = 0usize;
        for (i, plan) in plans.iter().enumerate() {
            preparation_checkpoint(stop)?;
            let bytes = plan.report_length()?;
            items = items
                .checked_add(usize::from(i != 0))
                .and_then(|n| n.checked_add(bytes))
                .ok_or(CapacityError::Overflow)?;
            bounded(items, MAX_METADATA)?;
        }
        let history = with_items(history_base, items, MAX_METADATA)?;
        let report = with_items(report_base, items, MAX_METADATA)?;
        let intent = bounded(canonical_json(self.job)?.len(), MAX_METADATA)?;
        for (name, bytes) in [
            (TARGET_INTENT, intent),
            (inspection::HISTORY_COMPLETE, history),
            (inspection::COMPLETE, report),
        ] {
            account(EntryName::new(name)?, bytes)?;
        }
        if let Some(batch) = self.batch {
            let whole = batch_length(batch, report)?;
            account(EntryName::new(batch::BATCH_INTENT)?, batch.intent)?;
            account(EntryName::new(batch::BATCH_COMPLETE)?, whole)?;
        }
        Ok(())
    }
}

fn bounded(bytes: usize, maximum: usize) -> Result<usize> {
    if bytes > maximum {
        return Err(CapacityError::RecordLimit.into());
    }
    Ok(bytes)
}

fn with_items(base: usize, items: usize, maximum: usize) -> Result<usize> {
    bounded(
        base.checked_add(items).ok_or(CapacityError::Overflow)?,
        maximum,
    )
}

fn batch_length(batch: batch::BatchEnvelope, report: usize) -> Result<usize> {
    // An admitted batch has one identical frozen source; varying per-target
    // job hashes are all fixed-width hex. Each target stores the whole batch receipt.
    let items = report
        .checked_mul(batch.targets)
        .and_then(|n| n.checked_add(batch.targets.saturating_sub(1)))
        .ok_or(CapacityError::Overflow)?;
    with_items(batch.completion_base, items, batch::MAX_BATCH_BYTES)
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/backup_copy_record_cost_tests.rs"]
mod tests;
