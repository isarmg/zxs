//! Cold whole-group observation, not permission to complete the original transaction.
use super::{FreshGenesis, Root, Selection, acquire, finish_roots, invalid, sample_roots};
use crate::maintenance::pending::ColdPending;
use std::io;

/// Owns both the original named registration EX and every explicit original volume EX.
/// No caller can extract a descriptor, complete only the first eligible row, or write a label.
#[must_use]
pub(in crate::maintenance) struct PendingRootSet {
    source: ColdPending, roots: Vec<Root>, labels: Vec<bool>,
}

impl PendingRootSet {
    pub(in crate::maintenance) fn inspect(source: ColdPending, selected: &Selection) -> io::Result<Self> {
        let uid = source.storage_uid();
        let image = source.verified_genesis()?;
        let roots = acquire(selected, uid, Some(&image))?; // all nonblocking, actual original order
        let labels = inspect_locked(&source, &roots, uid, &image)?;
        source.recheck()?;
        Ok(Self { source, roots, labels })
    }

    pub(in crate::maintenance) fn recheck(&self) -> io::Result<()> {
        let image = self.source.verified_genesis()?;
        let labels = inspect_locked(&self.source, &self.roots, self.source.storage_uid(), &image)?;
        if labels != self.labels { return Err(invalid("an original pending volume label changed since this observation")); }
        self.source.recheck()
    }
}

fn inspect_locked(source: &ColdPending, roots: &[Root], uid: u32, image: &FreshGenesis<'_>) -> io::Result<Vec<bool>> {
    let samples = sample_roots(roots, uid, Some(image))?;
    finish_roots(roots, uid, Some(image), &samples)?;
    // acquire already requires the complete, nonempty S/R/F set in row order.
    source.check_system_observation(&roots[0].observed, samples[0].labelled)?;
    Ok(samples.iter().map(|s| s.labelled).collect())
}
