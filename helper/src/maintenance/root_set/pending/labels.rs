//! Apply only this persistent original request. No public command or operator
//! grant is implemented. An error can leave an honest partial set of labels;
//! never remove a label, overwrite another owner, or generate replacement IDs.
mod publish;
use super::{FreshGenesis, PendingRootSet, RegisterName};
use crate::{maintenance::{ancestry::Mounts, thread_evidence::WorkerPolicy}, pinned::invalid};
use rustix::fs::{XattrFlags, fsetxattr};
use std::io;

/// No external producer. Carries the actual requested SYSTEM label bytes and is
/// created only when the CREATE and the original root's sync both returned success.
pub(in crate::maintenance) struct SystemLabelCreated<'a>(&'a [u8]);
impl SystemLabelCreated<'_> {
    pub(in crate::maintenance) fn bytes(&self) -> &[u8] { self.0 }
}

/// Constructed here only after every actual original volume was synchronized
/// and its exact label read back in this call. A cold observation of even a
/// complete set does not supply this object. Still not authorization or READY.
#[must_use]
struct SyncedLabels<'r>(PendingRootSet<'r>);
impl SyncedLabels<'_> {
    fn recheck(&self) -> io::Result<()> {
        if self.0.labels.iter().any(|present| !present) { return Err(invalid("incomplete synchronized label set")); }
        self.0.recheck()
    }
}
impl<'r> PendingRootSet<'r> {
    // A read-only check never calls this method; the outward maintenance
    // dispatcher must not enable it before real original-request authorization.
    fn apply_labels(mut self, op: [u8; 16], worker: &WorkerPolicy) -> io::Result<SyncedLabels<'r>> {
        let wanted = self.prepare_labels(op, worker)?;
        for (i, label) in wanted.iter().enumerate() { self.apply_one(i, label, worker)?; }
        let synced = SyncedLabels(self);
        synced.recheck()?; // all original names, entries, labels, register and frontier together
        Ok(synced)
    }

    fn prepare_labels(&self, op: [u8; 16], worker: &WorkerPolicy) -> io::Result<Vec<Vec<u8>>> {
        worker.require(self.source.storage_uid())?;
        let g = self.source.verified_genesis()?;
        if !g.has_init_operation(&op) { return Err(invalid("label application needs the original initialization operation")); }
        let wanted = (0..self.labels.len()).map(|i| g.expected_label(i)
            .map_err(|_| invalid("missing original volume row"))).collect::<io::Result<Vec<_>>>()?;
        self.recheck()?; // later unknown row => not one earlier label is written
        self.source.ensure_name_durable()?;
        self.recheck()?;
        Ok(wanted)
    }

    fn check_row(&self, i: usize, expected: bool, wanted: &[u8]) -> io::Result<()> {
        use super::super::{finish_one, sample_one};
        let g: FreshGenesis<'_> = self.source.verified_genesis()?;
        g.check_label(i, wanted).map_err(|_| invalid("not the original volume label"))?;
        let r = self.roots.all().get(i).ok_or_else(|| invalid("unknown original volume row"))?;
        let uid = self.source.storage_uid();
        let before = sample_one(&Mounts::current()?, r, uid, Some((&g, RegisterName::Pending)), i)?;
        finish_one(&Mounts::current()?, r, uid, Some(&g), i, &before)?;
        if before.labelled != expected { return Err(invalid("unexpected original label change")); }
        if i == 0 { self.source.check_system_observation(&r.observed, expected)?; }
        self.source.recheck()
    }

    fn apply_one(&mut self, i: usize, wanted: &[u8], worker: &WorkerPolicy) -> io::Result<()> {
        let existed = *self.labels.get(i).ok_or_else(|| invalid("missing original label observation"))?;
        self.check_row(i, existed, wanted)?;
        worker.require(self.source.storage_uid())?;
        let dir = &self.roots.all()[i].dir; // actual original, continuously locked by this whole group
        if !existed {
            fsetxattr(dir, c"user.zxs.volume", wanted, XattrFlags::CREATE)?;
        }
        // Even exact existing labels may predate the prior call's successful
        // directory sync: never infer durability from their mere presence.
        dir.sync_all()?;
        if !existed && i == 0 {
            self.source.observe_own_system_label(SystemLabelCreated(wanted))?;
        }
        self.check_row(i, true, wanted)?;
        self.labels[i] = true;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/maintenance/volume_labels.rs"]
mod tests;
