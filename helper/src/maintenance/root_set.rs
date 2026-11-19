//! Taking the selected directory locks allocates no registration. A separate
//! internal proposal can allocate one unnamed inode; it writes no bytes or
//! attributes, publishes no name and issues no operation receipt.
mod proposal;
pub(super) use proposal::UnpublishedProposal;
use super::{ancestry::Mounts, genesis::FreshGenesis, identity::ObservedOriginal};
use crate::pinned::{Kind, Pinned, invalid, storage_status::Selection};
use rustix::fs::{FlockOperation::NonBlockingLockExclusive, RawDir, flock};
use std::{fs::File, io, mem::MaybeUninit, os::unix::{ffi::OsStrExt, fs::MetadataExt}};

struct Root { name: Pinned, dir: File, observed: ObservedOriginal, component_limit: u8 }

// Directory contents and claims may change even under an advisory flock.
// Do not count reads through NOATIME as changes or treat a stamp as a lock.
#[derive(PartialEq, Eq)]
struct DirectoryStamp { links: u64, size: u64, mtime: (i64, i64), ctime: (i64, i64) }
struct RootSample { stamp: DirectoryStamp, mounts: Vec<u64> }
impl DirectoryStamp {
    fn on(dir: &File) -> io::Result<Self> {
        let m = dir.metadata()?;
        if !m.is_dir() || m.nlink() == 0 { return Err(invalid("storage directory was unlinked")); }
        Ok(Self { links: m.nlink(), size: m.len(), mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()) })
    }
}

impl Root {
    fn checked_name(&self, uid: u32) -> io::Result<File> {
        let named = self.name.storage_directory()?;
        if ObservedOriginal::root(&named, uid)? != self.observed
            || ObservedOriginal::root(&self.dir, uid)? != self.observed
            || helper_linux_evidence::component_limit(&self.dir)? != self.component_limit {
            return Err(invalid("selected directory, mount, owner or component limit changed"));
        }
        Ok(named)
    }
}

/// Only held locks and this fresh observation, never a permit for initialization.
pub(super) struct FreshRootLocks { roots: Vec<Root>, uid: u32 }
impl FreshRootLocks {
    pub(super) fn try_lock(selection: &Selection, storage_uid: u32) -> io::Result<Self> {
        // Selection is bounded to SYSTEM, recycle and 1..62 data roots. Every
        // configured name gets an independent description: no inherited lock conversion.
        let mut roots = Vec::new();
        for (_, path) in selection.paths() {
            let name = Pinned::open(path, Kind::StorageRoot)?;
            let dir = name.storage_directory()?;
            let observed = ObservedOriginal::root(&dir, storage_uid)?;
            let component_limit = helper_linux_evidence::component_limit(&dir)?;
            roots.push(Root { name, dir, observed, component_limit });
        }
        if !(3..=64).contains(&roots.len()) { return Err(invalid("invalid number of storage roots")); }
        // Record order is NOT lock order. Identical prefixes without a component
        // boundary are fine. The full original ancestry is checked with all locks held.
        roots[2..].sort_by(|a,b| a.name.path().as_os_str().as_bytes().cmp(b.name.path().as_os_str().as_bytes()));
        for (i, a) in roots.iter().enumerate() {
            for (j, b) in roots[..i].iter().enumerate() {
                if a.observed.at.entity == b.observed.at.entity || a.observed.birth == b.observed.birth
                    || a.name.entity_is_visible_ancestor_of(&b.name) || b.name.entity_is_visible_ancestor_of(&a.name) {
                    return Err(invalid("aliased original or nested visible storage roots"));
                }
                if (i == 1 || j == 1) && (a.observed.at.entity.dev == b.observed.at.entity.dev
                    || a.observed.birth.filesystem_uuid() == b.observed.birth.filesystem_uuid()) {
                    return Err(invalid("recycle needs a distinct device and filesystem UUID from SYSTEM and every data root"));
                }
            }
        }
        // Full Birth payload, with NO length prefix, then unsigned numeric dev/ino.
        let key = |r: &Root| (r.observed.at.entity.dev, r.observed.at.entity.ino);
        let mut order: Vec<_> = (0..roots.len()).collect();
        order.sort_unstable_by(|&a,&b| roots[a].observed.birth.payload().cmp(roots[b].observed.birth.payload())
            .then_with(|| key(&roots[a]).cmp(&key(&roots[b]))));
        for i in order { flock(&roots[i].dir, NonBlockingLockExclusive)?; }
        // Every owned OFD closes together on any failure, including post-lock failure.
        let locked = Self { roots, uid: storage_uid };
        locked.check_empty_selection()?;
        Ok(locked)
    }

    fn sample_empty_roots(&self) -> io::Result<Vec<RootSample>> {
        let mounts = Mounts::current()?;
        self.roots.iter().map(|r| {
            let named = r.checked_name(self.uid)?;
            let before = DirectoryStamp::on(&named)?;
            let chain = mounts.unclaimed_chain(&r.name, &named)?;
            read_empty(named)?;
            Ok(RootSample { stamp: before, mounts: chain })
        }).collect()
    }

    // A genuine EOF is not evidence that the original is still named: the
    // selected directory, even a previously scanned row, may have disappeared.
    fn finish_empty_roots(&self, samples: &[RootSample]) -> io::Result<()> {
        if samples.len() != self.roots.len() { return Err(invalid("incomplete storage root observation")); }
        let mounts = Mounts::current()?;
        for (r, before) in self.roots.iter().zip(samples) {
            let named = r.checked_name(self.uid)?;
            if DirectoryStamp::on(&named)? != before.stamp || DirectoryStamp::on(&r.dir)? != before.stamp
                || mounts.unclaimed_chain(&r.name, &named)? != before.mounts {
                return Err(invalid("storage directory changed during observation"));
            }
        }
        Ok(())
    }

    fn check_empty_selection(&self) -> io::Result<()> {
        let samples = self.sample_empty_roots()?;
        self.finish_empty_roots(&samples)
    }

    /// Check all observed volume origins/role positions, component limits and path bytes, not
    /// prospective VolumeIds or future first-name publication. Cannot expose
    /// a directory FD to a writer or adopt a hidden ancestor.
    pub(super) fn check_image_roots(&self, image: &FreshGenesis<'_>) -> io::Result<()> {
        self.check_empty_selection()?;
        if !image.has_root_count(self.uid, self.roots.len()) || self.roots.iter().enumerate().any(|(i,r)|
            !image.matches_root_origin(i, r.name.path().as_os_str().as_bytes(), r.component_limit, r.observed.fields())) {
            return Err(invalid("new image differs from the held original root selection"));
        }
        Ok(())
    }
}

// A newly opened directory description starts at offset zero on each scan;
// never advance the original locked description. RawDir preserves getdents
// errors (including ENOENT on removal); only its real end counts as EOF.
fn read_empty(dir: File) -> io::Result<()> {
    let mut buffer = [MaybeUninit::<u8>::uninit(); 4096];
    let mut stream = RawDir::new(dir, &mut buffer);
    let mut dot = false;
    let mut parent = false;
    while let Some(entry) = stream.next() {
        let entry = entry?;
        match entry.file_name().to_bytes() {
            b"." if !dot => dot = true,
            b".." if !parent => parent = true,
            _ => return Err(invalid("storage root is not empty or its entry stream is inconsistent")),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/maintenance/root_set.rs"]
mod tests;
