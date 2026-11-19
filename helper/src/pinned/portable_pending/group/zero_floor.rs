//! Only the original empty CONFIRMED. No initial IDs, volume markers or READY publication.
use super::*;

/// Only this successful create-or-resync and named readback produces this
/// state. Merely observing a zero floor cannot construct it. REG and all root
/// locks remain held; no writer/FD or operator receipt is returned.
#[must_use]
pub(super) struct SyncedFloor<'a>(LockedGroup<'a>);
impl SyncedFloor<'_> {
    pub(super) fn recheck(&self) -> io::Result<()> { self.0.verify() }
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
            existing.fd.sync_all()?;
            &existing.fd
        } else {
            created=File::from(openat(s,OsStr::from_bytes(FLOOR),
                OFlags::CREATE|OFlags::EXCL|OFlags::RDWR|OFlags::NOFOLLOW|OFlags::CLOEXEC,
                Mode::RUSR|Mode::WUSR)?);
            let fresh=Stamp::on(&created)?;
            if !fresh.kind.is_file() || fresh.uid!=self.source.uid || fresh.mode&0o7777!=0o600
                || fresh.links!=1 || fresh.len!=0 || fresh.mount!=parent.mount
                || fresh.entity.dev!=parent.entity.dev || Stamp::on(&pin(s,FLOOR)?)?!=fresh {
                return Err(bad("new confirmation is not the expected empty private original; preserved"));
            }
            let names=Names::read(self.roots[0].name.storage_directory()?)?;
            if !names.floor || names.marker { return Err(bad("unexpected original names after exclusive create")); }
            created.write_all_at(&exact,0)?;
            created.sync_all()?;
            &created
        };
        s.sync_all()?;
        let observed=Record::open(s,&Stamp::root(s,self.source.uid)?,FLOOR)?;
        if Stamp::on(original)?!=observed.original || observed.bytes()?.as_slice()!=exact.as_slice() {
            return Err(bad("confirmation did not read back from its exact original and parent"));
        }
        observed.require_name(s)?;
        self.source.floor=Some(observed); // sole admitted transition: our confirmed None→Some, or same existing original
        self.verify()?;
        Ok(SyncedFloor(self))
    }
}
