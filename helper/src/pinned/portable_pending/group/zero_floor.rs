//! Only the original empty CONFIRMED. No initial IDs, volume markers or READY publication.
use super::{feedback_file::Output, *};

/// Only this successful create-or-resync and named readback produces this
/// state. Merely observing a zero floor cannot construct it. REG and all root
/// locks remain held; no writer/FD or operator receipt is returned.
#[must_use]
pub(super) struct SyncedFloor<'a>(LockedGroup<'a>);
impl<'a> SyncedFloor<'a> {
    pub(super) fn recheck(&self) -> io::Result<()> { self.0.verify() }
    // No constructor outside this module. Moving this value is the only way
    // the marker operation receives a group with a genuinely synced floor.
    pub(super) fn into_group(self) -> LockedGroup<'a> { self.0 }
}

impl<'a> LockedGroup<'a> {
    pub(super) fn persist_zero(mut self) -> io::Result<SyncedFloor<'a>> {
        self.order.worker.require(self.source.uid)?;
        self.verify()?;
        // A prior process may have lost its response before the initial
        // fsyncs. A cold read alone never proved that first name durable.
        self.source.register.fd.sync_all()?;
        self.roots[0].dir.sync_all()?;
        self.verify()?;
        let reg=self.source.register.bytes()?;
        let initial=Initial::decode(&reg).map_err(|_| bad("initial request is no longer intact"))?;
        initial.check_request(self.order.request).map_err(|_| bad("wrong operation for zero confirmation"))?;
        let exact=initial.zero_floor();
        let s=&self.roots[0].dir;
        let parent=Stamp::root(s,self.source.uid)?;
        self.order.worker.require(self.source.uid)?;
        // Once creation has been attempted, failures retain all evidence. No
        // truncate/unlink, repair of a partial file, or retry through another name.
        let created;
        let original=if let Some(existing)=&self.source.floor {
            existing.require_name(s)?;
            &existing.fd
        } else {
            created=Output::Floor.create(s)?;
            let names=Names::read(self.roots[0].name.storage_directory()?)?;
            if !names.floor || names.marker { return Err(bad("unexpected original names after exclusive create")); }
            Output::Floor.require_new(s,&parent,&created)?;
            created.write_all_at(&exact,0)?;
            &created
        };
        let observed=Output::Floor.sync_and_readback(s,self.source.uid,original,&exact)?;
        self.source.floor=Some(observed); // sole admitted transition: our confirmed None→Some, or same existing original
        self.verify()?;
        Ok(SyncedFloor(self))
    }
}
