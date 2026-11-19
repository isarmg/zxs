//! Publish precisely this fully synchronized PENDING, or explicitly reconfirm
//! the same already-published empty request. Neither path issues a new ID.
use super::{feedback_file::Output,markers::MarkerRun,*};
use rustix::fs::{RenameFlags,renameat_with};

#[derive(Debug)]
pub(super) enum PublishError {
    BeforeRename(io::Error),
    /// A rename was attempted: retain both names as observed, do not undo,
    /// clean up, reissue IDs or automatically attempt a second publication.
    InspectOriginal(io::Error),
}

#[must_use]
pub(super) struct ConfirmedOriginal<'a>(PublishedGroup<'a>);

impl ConfirmedOriginal<'_> {
    pub(super) fn recheck(&self) -> io::Result<()> { self.0.verify() }
}

// Unlike a regular read cut, our own same-parent rename can change the
// original inode's ctime. It cannot replace or rewrite this still-held file.
fn same_across_rename(before:&Stamp, after:&Stamp) -> bool {
    before.entity==after.entity && before.mount==after.mount && before.kind==after.kind
        && before.uid==after.uid && before.mode==after.mode && before.links==after.links
        && before.len==after.len && before.modified==after.modified
}

impl<'a> MarkerRun<'a> {
    pub(super) fn publish(self) -> Result<ConfirmedOriginal<'a>,PublishError> {
        let pending=self.into_fully_synced().map_err(PublishError::BeforeRename)?;
        let bytes=pending.check_for_publication().map_err(PublishError::BeforeRename)?;
        let system=&pending.roots[0].dir;
        // NOREPLACE protects the target, not the source inode against an
        // uncooperative writer. The private parent and writer policy are essential.
        renameat_with(system,OsStr::from_bytes(PENDING),system,OsStr::from_bytes(READY),RenameFlags::NOREPLACE)
            .map_err(|e|PublishError::InspectOriginal(e.into()))?;
        pending.after_rename(&bytes).map_err(PublishError::InspectOriginal)
    }
}

impl<'a> LockedGroup<'a> {
    fn check_for_publication(&self) -> io::Result<Vec<u8>> {
        self.order.worker.require(self.source.uid)?;
        self.verify()?;
        if self.source.floor.is_none() || self.source.marker.is_none() ||
           self.roots.iter().skip(1).any(|r|r.marker.is_none()) {
            return Err(bad("all original volume claims are required before publication"));
        }
        let bytes=self.source.register.bytes()?;
        let reg=Initial::decode(&bytes).map_err(|_|bad("not the original empty register"))?;
        reg.check_request(self.order.request).map_err(|_|bad("different approved operation"))?;
        let system=&self.roots[0];
        system.require_origin(self.source.uid)?;
        let names=Names::for_name(system.name.storage_directory()?,PENDING)?;
        if !names.floor || !names.marker { return Err(bad("publication requires both original SYSTEM feedback files")); }
        match pin(&system.dir,READY) {
            Err(e) if e.kind()==io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
            Ok(_) => return Err(bad("publication destination already exists")),
        }
        self.order.worker.require(self.source.uid)?;
        self.source.register.require_name(&system.dir)?;
        Ok(bytes)
    }

    fn after_rename(self, bytes:&[u8]) -> io::Result<ConfirmedOriginal<'a>> {
        let RootSession {source,order,roots}=self;
        let root=&roots[0];
        let dir=&root.dir;
        let parent=root.require_origin(source.uid)?;
        let post=Stamp::on(&source.register.fd)?;
        let names=Names::for_name(root.name.storage_directory()?,READY)?;
        if !names.floor || !names.marker || !same_across_rename(&source.register.original,&post)
            || Stamp::on(&pin(dir,READY)?)?!=post {
            return Err(bad("published name does not identify the original locked register"));
        }
        // Capture both objects after the allowed rename and before syncing.
        // The long EX on the register is continuous: no new flock, no release.
        source.register.fd.sync_all()?;
        dir.sync_all()?;
        let observed=Record::open(dir,&parent,READY)?;
        if observed.original!=post || observed.bytes()?.as_slice()!=bytes {
            return Err(bad("published original did not read back its complete request"));
        }
        observed.require_name(dir)?;
        if Stamp::on(&source.register.fd)?!=post || root.require_origin(source.uid)?!=parent {
            return Err(bad("original register or parent changed across publication synchronization"));
        }
        // Keep the locked original OFD, NOT the independent descriptor just
        // used for readback. Its published name and post-rename stamp are now known.
        let published=HeldReady {
            path:source.path, system:source.system, uid:source.uid, floor:source.floor, marker:source.marker,
            register:Record { name:READY, fd:source.register.fd, original:post },
        };
        let completed=PublishedGroup { source:published,order,roots };
        completed.verify()?;
        Ok(ConfirmedOriginal(completed))
    }
}

impl<'a> PublishedGroup<'a> {
    /// Not the pure single-S observer. An explicit matching full order and all
    /// original root EXs are required. Missing files are never created here.
    pub(super) fn confirm(held:HeldReady, order:FloorOrder<'a>) -> io::Result<ConfirmedOriginal<'a>> {
        let group=Self::acquire(held,order)?; // proves all origins, original request and every marker BEFORE any sync
        let source=&group.source;
        let bytes=source.register.bytes()?;
        let image=Initial::decode(&bytes).map_err(|_|bad("not one published empty register"))?;
        image.check_request(group.order.request).map_err(|_|bad("not the original approved confirmation"))?;
        group.order.worker.require(source.uid)?;
        for (row,root) in group.roots.iter().enumerate().skip(1) {
            let marker=root.marker.as_ref().ok_or_else(||bad("missing original published volume claim"))?;
            let claim=image.row(row).map_err(|_|bad("missing original published row"))?.2.encode();
            root.require_origin(source.uid)?;
            if !root.data_names()? || marker.bytes()?.as_slice()!=claim.as_slice() {
                return Err(bad("non-system volume no longer carries its exact original claim"));
            }
            marker.require_name(&root.dir)?;
            let read=Output::Marker.sync_and_readback(&root.dir,source.uid,&marker.fd,&claim)?;
            if read.original!=marker.original { return Err(bad("original published marker changed")); }
            root.require_origin(source.uid)?;
        }
        // The actual SYSTEM parent must be the last directory sync, after
        // all three SYSTEM files. The later operations are observations only.
        group.order.worker.require(source.uid)?;
        group.verify()?;
        let system=&group.roots[0];
        let parent=system.require_origin(source.uid)?;
        let floor=source.floor.as_ref().ok_or_else(||bad("published zero floor absent"))?;
        let marker=source.marker.as_ref().ok_or_else(||bad("published SYSTEM claim absent"))?;
        let fs=image.zero_floor();
        let ms=image.row(0).map_err(|_|bad("missing SYSTEM row"))?.2.encode();
        let records=[(&source.register,bytes.as_slice()),(floor,fs.as_slice()),(marker,ms.as_slice())];
        for (r,expected) in records {
            r.require_name(&system.dir)?;
            if r.bytes()?.as_slice()!=expected { return Err(bad("SYSTEM feedback differs from this original")); }
            r.fd.sync_all()?;
        }
        system.dir.sync_all()?;
        for (r,expected) in records {
            let post=Record::open(&system.dir,&parent,r.name)?;
            if post.original!=r.original || post.bytes()?.as_slice()!=expected {
                return Err(bad("SYSTEM confirmation does not read back the synchronized original"));
            }
            post.require_name(&system.dir)?;
            r.require_name(&system.dir)?;
        }
        if system.require_origin(source.uid)?!=parent { return Err(bad("SYSTEM changed across final confirmation")); }
        group.verify()?;
        Ok(ConfirmedOriginal(group))
    }
}
