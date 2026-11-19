//! In original S/R/F order, synchronize one volume claim per explicit step.
//! An exact visible prefix is NOT evidence of a prior file or parent sync.
use super::{feedback_file::Output, zero_floor::SyncedFloor, *};

/// No raw descriptor, caller-selected index, READY name or operation receipt.
/// A failure consumes the cursor; actual effects are retained for a cold check.
#[must_use]
pub(super) struct MarkerRun<'a> { group: LockedGroup<'a>, next: usize }

impl<'a> SyncedFloor<'a> {
    pub(super) fn markers(self) -> MarkerRun<'a> {
        // Even if all marks are visible after a crash, begin at row 0.
        MarkerRun { group: self.into_group(), next: 0 }
    }
}

impl MarkerRun<'_> {
    pub(super) fn synced_rows(&self) -> usize { self.next }

    fn inventory(&self, row: usize, present: bool) -> io::Result<()> {
        let root=&self.group.roots[row];
        let actual=if row==0 {
            let system=Names::read(root.name.storage_directory()?)?;
            if !system.floor { return Err(bad("missing original zero confirmation on SYSTEM")); }
            system.marker
        } else { root.data_names()? };
        if actual!=present { return Err(bad("volume marker name or inventory changed; preserved")); }
        Ok(())
    }

    pub(super) fn sync_next(mut self) -> io::Result<Self> {
        let row=self.next;
        if row>=self.group.roots.len() { return Err(bad("no unsynchronized row in this original request")); }
        self.group.order.worker.require(self.group.source.uid)?;
        // The caller may hold this cursor between steps. Recheck every
        // original before touching the next one, including yet-unmarked roots.
        self.group.verify()?;
        let source=&self.group.source;
        let bytes=source.register.bytes()?;
        let image=Initial::decode(&bytes).map_err(|_| bad("initial request is no longer intact"))?;
        image.check_request(self.group.order.request).map_err(|_| bad("wrong original request for volume claim"))?;
        let (_,_,claim)=image.row(row).map_err(|_| bad("missing original volume row"))?;
        let expected=claim.encode();
        let uid=source.uid;
        let root=&self.group.roots[row];
        let parent=root.require_origin(uid)?;
        let existing=if row==0 { &source.marker } else { &root.marker };
        self.inventory(row,existing.is_some())?;
        self.group.order.worker.require(uid)?;

        let created;
        let original=if let Some(record)=existing {
            record.require_name(&root.dir)?;
            claim.check_row(&record.bytes()?).map_err(|_| bad("original volume claim differs; preserved"))?;
            &record.fd
        } else {
            created=Output::Marker.create(&root.dir)?;
            self.inventory(row,true)?; // exact raw name, no extra/foreign directory entry
            root.require_origin(uid)?;
            Output::Marker.require_new(&root.dir,&parent,&created)?;
            created.write_all_at(&expected,0)?;
            &created
        };
        let observed=Output::Marker.sync_and_readback(&root.dir,uid,original,&expected)?;
        if row==0 { self.group.source.marker=Some(observed); }
        else {
            // Close the SYSTEM sample first, then finish on the non-S root's
            // exact name and still-held file before counting this row.
            let system=&self.group.roots[0].dir;
            self.group.source.sample(system,&Stamp::root(system,uid)?,self.group.order.request)?;
            self.inventory(row,true)?;
            self.group.roots[row].require_origin(uid)?;
            observed.require_name(&self.group.roots[row].dir)?;
            self.group.roots[row].marker=Some(observed);
        }
        if row==0 {
            let system=&self.group.roots[0].dir;
            self.group.source.sample(system,&Stamp::root(system,uid)?,self.group.order.request)?;
        }
        self.next+=1;
        if self.next==self.group.roots.len() { self.group.verify()?; }
        Ok(self)
    }
}
