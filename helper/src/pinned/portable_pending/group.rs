//! Full, explicitly supplied cold selection. No saved path supplies an origin.
//! A storage worker's OS identity is not approval of a particular operation.
mod zero_floor;

use super::*;
use crate::maintenance::{HostMounts, WorkerPolicy};

/// Private handoff from the future protected approval path. The current project
/// has NO production issuer. The host root must be held by the trusted deployment
/// independently of the worker's visible "/" (not rediscovered from mountinfo).
/// Origin handles must refer to approved ordinary storage, not a virtual re-export
/// or unknown backing store. Neither the request nor process credentials attest this.
struct FloorOrder<'a> {
    request: &'a [u8],
    origins: &'a [(&'a Path, &'a File)], // exact original S, R, sorted F; never derived from media
    host_root: &'a File,
    worker: &'a WorkerPolicy,
}

struct Root<'a> {
    original: &'a File,
    name: Pinned,
    dir: File, // fresh OFD; held EX after collection
    marker: Option<Record>, // only for non-S roots; S is held by HeldPending
}

fn located(f: &File) -> io::Result<(Entity, Mount)> {
    let mount = Mount::on(f)?.ok_or_else(|| unsupported("no live mount identity"))?;
    Ok((Entity::of(&f.metadata()?)?, mount))
}

// The format permits shorter limits so an existing request can still be
// inspected. Never advance one known to be unable to hold a mandatory marker.
fn require_marker_space(recorded:u8, current:u8) -> io::Result<()> {
    if current!=recorded { return Err(bad("component budget differs from the original request")); }
    if usize::from(current)<ROOT_SLOT.len() { return Err(unsupported("selected root cannot hold the required volume marker name")); }
    Ok(())
}

fn has_reserved_name(dir: &File) -> io::Result<bool> {
    match openat(dir, OsStr::from_bytes(ROOT_SLOT), OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty()) {
        Ok(_) => Ok(true), // never read or interpret an unselected ancestor's entry
        Err(rustix::io::Errno::NOENT) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

impl Root<'_> {
    fn require_origin(&self, uid: u32) -> io::Result<Stamp> {
        let s = Stamp::root(&self.dir, uid)?;
        if Stamp::root(&self.original, uid)? != s || Stamp::root(&self.name.storage_directory()?, uid)? != s {
            return Err(bad("a selected volume no longer matches its supplied original directory"));
        }
        Ok(s)
    }

    /// Both directions must name the same original directories. A whole mount
    /// must cross at its observed root to its kernel parent. Reject subtree
    /// binds and missing relations. At "/" require the independent deployment
    /// anchor, not a missing parent row or a self-reported 'not chrooted' flag.
    fn ancestry(&self, table: &HostMounts, host_root: &File) -> io::Result<u64> {
        let mut ancestors = self.name.visible_chain().iter().rev().peekable();
        let mut up = None;
        let mut below: Option<(Mount,u64)> = None;
        let mut leaf_superblock = None;
        while let Some(expected) = ancestors.next() {
            let at = up.as_ref().unwrap_or(&self.dir);
            let (id, mount) = located(at)?;
            let meta = at.metadata()?;
            if id != *expected || !meta.is_dir() || meta.nlink()==0 {
                return Err(bad("real ancestry differs from the selected path"));
            }
            let (parent, superblock) = table.full_mount(mount.id)?;
            if let Some((child, expected_parent)) = below {
                if child.id != mount.id && (child.at_root != Some(true) || expected_parent != mount.id) {
                    return Err(unsupported("cannot attribute a mount crossing in the selected ancestry"));
                }
                if has_reserved_name(at)? { return Err(bad("cannot establish that a selected ancestor is unclaimed")); }
            } else { leaf_superblock = Some(superblock); }
            if ancestors.peek().is_none() {
                if mount.at_root != Some(true) || located(host_root)? != (id,mount) {
                    return Err(unsupported("ancestry did not reach the independently held deployment root"));
                }
                break;
            }
            below = Some((mount,parent));
            up = Some(File::from(openat(at,c"..",OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,Mode::empty())?));
        }
        leaf_superblock.ok_or_else(|| bad("missing root identity"))
    }

    fn data_names(&self) -> io::Result<bool> {
        let dir = self.name.storage_directory()?;
        let mut buf = [MaybeUninit::<u8>::uninit();4096];
        let mut scan = RawDir::new(dir,&mut buf);
        let (mut dot,mut parent,mut marker)=(false,false,false);
        while let Some(entry)=scan.next() {
            match entry?.file_name().to_bytes() {
                b"." if !dot => dot=true,
                b".." if !parent => parent=true,
                ROOT_SLOT if !marker => marker=true,
                _ => return Err(bad("unknown, repeated or non-exact name on a pending data or recycle root")),
            }
        }
        Ok(marker)
    }
}

struct LockedGroup<'a> { source: HeldPending, order: FloorOrder<'a>, roots: Vec<Root<'a>> }

impl<'a> LockedGroup<'a> {
    fn acquire(source:HeldPending, order:FloorOrder<'a>) -> io::Result<Self> {
        order.worker.require(source.uid)?;
        // Re-establish the original S and exact request before the first open
        // of even a caller-selected R/F. This short SH is gone before root EXs.
        source.recheck(order.request)?;
        let bytes = source.register.bytes()?;
        let original = Initial::decode(&bytes).map_err(|_| bad("not an empty portable pending request"))?;
        original.check_request(order.request).map_err(|_| bad("wrong original request"))?;
        if order.origins.len() != original.row_count() { return Err(bad("incomplete or extra host selection")); }
        let mut roots = Vec::<Root>::with_capacity(order.origins.len());
        for (i,&(selected,anchor)) in order.origins.iter().enumerate() {
            let (path,limit,_) = original.row(i).map_err(|_| bad("missing original row"))?;
            if selected.as_os_str().as_bytes()!=path { return Err(bad("role order or explicit selected path differs from the original")); }
            let name = Pinned::open(selected,Kind::StorageRoot)?;
            let held = Stamp::root(anchor,source.uid)?;
            if Stamp::root(&name.fd,source.uid)? != held { return Err(bad("selected root does not match its separate host origin")); }
            // Before touching ANY other root, bind the group's row 0 to the
            // SYSTEM on which this very registration is already held exclusive.
            if i==0 && located(&name.fd)? != located(&source.system)? {
                return Err(bad("group SYSTEM differs from the locked pending request"));
            }
            let dir = name.storage_directory()?;
            if Stamp::root(&dir,source.uid)? != held { return Err(bad("changed original root")); }
            require_marker_space(limit,helper_linux_evidence::portable_component_limit(&dir)?)?;
            if roots.iter().any(|r| r.name.entity()==name.entity() || name.entity_is_visible_ancestor_of(&r.name)
                || r.name.entity_is_visible_ancestor_of(&name)) { return Err(bad("aliased or nested selected roots")); }
            roots.push(Root {original:anchor,name,dir,marker:None});
        }

        let mut locks:Vec<_> = (0..roots.len()).collect();
        // Both parts are live identities held by this call. Ordering is independent
        // of role, caller spelling or claimed IDs, including between different pools.
        locks.sort_unstable_by_key(|&i| { let e=roots[i].name.entity(); (e.dev,e.ino) });
        for i in locks { flock(&roots[i].dir,FlockOperation::NonBlockingLockExclusive)?; }
        for r in roots.iter_mut().skip(1) {
            let s=r.require_origin(source.uid)?;
            if r.data_names()? { r.marker=Some(Record::open(&r.dir,&s,ROOT_SLOT)?); }
        }
        let group=Self {source,order,roots};
        group.verify()?;
        Ok(group)
    }

    fn verify(&self) -> io::Result<()> {
        let s=&self.roots[0].dir;
        let source=&self.source;
        let stamps=self.roots.iter().map(|r| r.require_origin(source.uid)).collect::<io::Result<Vec<_>>>()?;
        // Own S EX already: do not call HeldPending::recheck() and conflict
        // with ourselves, or drop/downgrade the root lock for a 'read' step.
        source.sample(s,&stamps[0],self.order.request)?;
        let bytes=source.register.bytes()?;
        let original=Initial::decode(&bytes).map_err(|_| bad("invalid original request"))?;
        original.check_request(self.order.request).map_err(|_| bad("original approved request changed"))?;
        let table=HostMounts::current()?;
        let mut mounts=Vec::with_capacity(self.roots.len());
        let mut markers=Vec::with_capacity(self.roots.len());
        for (i,(r,stamp)) in self.roots.iter().zip(&stamps).enumerate() {
            let (path,limit,_)=original.row(i).map_err(|_| bad("missing original row"))?;
            if r.name.path().as_os_str().as_bytes()!=path { return Err(bad("volume path differs from its original row")); }
            require_marker_space(limit,helper_linux_evidence::portable_component_limit(&r.dir)?)?;
            mounts.push(r.ancestry(&table,self.order.host_root)?);
            let marker=if i==0 { &source.marker } else {
                if r.data_names()?!=r.marker.is_some() { return Err(bad("root marker presence changed")); }
                &r.marker
            };
            if let Some(m)=marker { m.require_name(&r.dir)?; }
            markers.push(marker.as_ref().map(Record::bytes).transpose()?);
            if Stamp::root(&r.dir,source.uid)?!=*stamp { return Err(bad("root changed during inspection")); }
        }
        // Only compare actual, simultaneously held, explicitly trusted storage
        // origins. Different live superblocks do not prove different hardware.
        for (i,r) in self.roots.iter().enumerate() {
            if i!=1 && (mounts[i]==mounts[1] || stamps[i].mount==stamps[1].mount
                || r.name.entity().dev==self.roots[1].name.entity().dev) {
                return Err(unsupported("recycler has no independently attributable filesystem"));
            }
        }
        let floor=source.floor.as_ref().map(Record::bytes).transpose()?;
        let marks=markers.iter().map(Option::as_deref).collect::<Vec<_>>();
        original.pending_feedback(floor.as_deref(),&marks).map_err(|_| bad("foreign or out-of-order feedback on a selected original"))?;
        let end=HostMounts::current()?;
        for (i,(r,before)) in self.roots.iter().zip(&stamps).enumerate() {
            if r.ancestry(&end,self.order.host_root)?!=mounts[i] || r.require_origin(source.uid)?!=*before {
                return Err(bad("volume identity, ancestry or contents changed during the group observation"));
            }
            if i!=0 {
                if r.data_names()?!=r.marker.is_some() { return Err(bad("volume inventory changed")); }
                if let Some(m)=&r.marker { m.require_name(&r.dir)?; }
            }
        }
        source.sample(s,&stamps[0],self.order.request)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/portable/resume_floor.rs"]
mod tests;
