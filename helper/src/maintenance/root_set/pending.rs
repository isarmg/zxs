//! Whole-group observation is not permission to complete the original transaction.
mod labels;
pub(in crate::maintenance) use labels::SystemLabelCreated;
use crate::maintenance::register_flock::staging::first_name::FirstPending;
use super::{FreshGenesis, FreshRootLocks, Root, Selection, acquire, finish_roots, invalid, sample_roots};
use crate::maintenance::{RegisterName, pending::ColdPending};
use std::io;

// A cold open owns newly acquired volume descriptions; a just-named original still
// borrows the already locked volumes. Neither reacquires its own EX or releases it early.
enum RootHolds<'r> { Cold(Vec<Root>), First(&'r FreshRootLocks) }
impl RootHolds<'_> {
    fn all(&self) -> &[Root] { match self { Self::Cold(roots) => roots, Self::First(group) => &group.roots } }
}

#[must_use]
pub(in crate::maintenance) struct PendingRootSet<'r> {
    source: ColdPending, roots: RootHolds<'r>, labels: Vec<bool>,
}

impl PendingRootSet<'static> {
    pub(in crate::maintenance) fn inspect(source: ColdPending, selected: &Selection) -> io::Result<Self> {
        let uid = source.storage_uid();
        let image = source.verified_genesis()?;
        let roots = acquire(selected, uid, Some(&image))?;
        let labels = inspect_locked(&source, &roots, uid, &image)?;
        source.recheck()?;
        Ok(Self { source, roots: RootHolds::Cold(roots), labels })
    }
}

impl<'r> PendingRootSet<'r> {
    pub(in crate::maintenance) fn from_first(first: FirstPending<'r>) -> io::Result<Self> {
        let (roots, source) = first.into_guards();
        roots.check_just_linked(&source)?;
        Ok(Self { source, roots: RootHolds::First(roots), labels: vec![false; roots.roots.len()] })
    }

    pub(in crate::maintenance) fn recheck(&self) -> io::Result<()> {
        let image = self.source.verified_genesis()?;
        let labels = inspect_locked(&self.source, self.roots.all(), self.source.storage_uid(), &image)?;
        if labels != self.labels { return Err(invalid("an original pending volume label changed since this observation")); }
        self.source.recheck()
    }
}
impl FreshRootLocks {
    // The first link alone claims no volume. Never absorb a concurrent (even
    // byte-identical) label or reacquire this transaction's own directory locks.
    pub(in crate::maintenance) fn check_just_linked(&self, source: &ColdPending) -> io::Result<()> {
        let g = source.verified_genesis()?;
        self.check_row_origins(&g)?;
        if inspect_locked(source, &self.roots, self.uid, &g)?.into_iter().any(|set| set) {
            return Err(invalid("a volume was already labelled at the first name"));
        }
        source.recheck()
    }
}

fn inspect_locked(source: &ColdPending, roots: &[Root], uid: u32, image: &FreshGenesis<'_>) -> io::Result<Vec<bool>> {
    let labels = inspect_volumes(roots, uid, image, RegisterName::Pending)?;
    // acquire already requires the complete, nonempty S/R/F set in row order.
    source.check_system_observation(&roots[0].observed, labels[0])?;
    Ok(labels)
}

fn inspect_volumes(roots: &[Root], uid: u32, image: &FreshGenesis<'_>, name: RegisterName) -> io::Result<Vec<bool>> {
    let samples = sample_roots(roots, uid, Some((image, name)))?;
    finish_roots(roots, uid, Some(image), &samples)?;
    Ok(samples.iter().map(|s| s.labelled).collect())
}
