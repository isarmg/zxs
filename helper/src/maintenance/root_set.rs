//! Taking the selected directory locks allocates no registration. A separate
//! internal proposal can allocate one unnamed inode; it writes no bytes or
//! attributes, publishes no name and issues no operation receipt.
mod proposal;
mod pending;
pub(super) use pending::{PendingRootSet, SystemLabelCreated};
pub(super) use proposal::UnpublishedProposal;
use super::{RegisterName, ancestry::Mounts, genesis::FreshGenesis, identity::ObservedOriginal};
use crate::pinned::{Kind, Pinned, invalid, storage_status::Selection};
use rustix::fs::{FlockOperation::NonBlockingLockExclusive, RawDir, flock};
use std::{fs::File, io, mem::MaybeUninit, os::unix::{ffi::OsStrExt, fs::MetadataExt}};

struct Root { name: Pinned, dir: File, observed: ObservedOriginal, component_limit: u8 }

// Directory contents and claims may change even under an advisory flock.
// Do not count reads through NOATIME as changes or treat a stamp as a lock.
#[derive(PartialEq, Eq)]
pub(super) struct DirectoryStamp { links: u64, size: u64, mtime: (i64, i64), ctime: (i64, i64) }
struct RootSample { stamp: DirectoryStamp, mounts: Vec<u64>, labelled: bool }
impl DirectoryStamp {
    pub(super) fn on(dir: &File) -> io::Result<Self> {
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

// Only caller-supplied paths are ever opened. Fresh and cold operations take the
// same independent directory locks in the same native order. The optional image
// can only narrow which explicit roots are eligible, never supply an alternative.
fn acquire(selection: &Selection, storage_uid: u32, original: Option<&FreshGenesis<'_>>) -> io::Result<Vec<Root>> {
    // Selection is bounded to SYSTEM, recycle and 1..62 data roots. Every
    // configured name gets an independent description: no inherited lock conversion.
    let mut roots = Vec::new();
    let mut paths: Vec<_> = selection.paths().map(|(_, path)| path).collect();
    if !(3..=64).contains(&paths.len()) { return Err(invalid("invalid number of storage roots")); }
    paths[2..].sort_unstable_by(|a,b| a.as_os_str().as_bytes().cmp(b.as_os_str().as_bytes()));
    if original.is_some_and(|g|!g.has_root_count(storage_uid, paths.len()) || paths.iter().enumerate().any(|(i,p)|
        !g.matches_request_path(i, p.as_os_str().as_bytes()))) {
        return Err(invalid("explicit SYSTEM, recycle and data selection differs from the pending request"));
    }
    for (i, path) in paths.into_iter().enumerate() {
        let name = Pinned::open(path, Kind::StorageRoot)?;
        let dir = name.storage_directory()?;
        let observed = ObservedOriginal::root(&dir, storage_uid)?;
        let component_limit = helper_linux_evidence::component_limit(&dir)?;
        if original.is_some_and(|g| !g.matches_root_origin(i, path.as_os_str().as_bytes(), component_limit, observed.fields())) {
            return Err(invalid("pending request matches a name but not the original volume"));
        }
        roots.push(Root { name, dir, observed, component_limit });
    }
    // Record order is NOT lock order. Identical prefixes without a component
    // boundary are fine. The full original ancestry is checked with all locks held.
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
    Ok(roots)
}

/// Only held locks and this fresh observation, never a permit for initialization.
pub(super) struct FreshRootLocks { roots: Vec<Root>, uid: u32 }
impl FreshRootLocks {
    pub(super) fn try_lock(selection: &Selection, storage_uid: u32) -> io::Result<Self> {
        let roots = acquire(selection, storage_uid, None)?;
        // Every owned OFD closes together on any failure, including post-lock failure.
        let locked = Self { roots, uid: storage_uid };
        locked.check_empty_selection()?;
        Ok(locked)
    }

    fn sample_empty_roots(&self) -> io::Result<Vec<RootSample>> {
        sample_roots(&self.roots, self.uid, None)
    }

    // A genuine EOF is not evidence that the original is still named: the
    // selected directory, even a previously scanned row, may have disappeared.
    fn finish_empty_roots(&self, samples: &[RootSample]) -> io::Result<()> {
        finish_roots(&self.roots, self.uid, None, samples)
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
        self.check_row_origins(image)
    }

    pub(super) fn selected_system(&self) -> &std::path::Path { self.roots[0].name.path() }

    fn check_row_origins(&self, image: &FreshGenesis<'_>) -> io::Result<()> {
        if !image.has_root_count(self.uid, self.roots.len()) || self.roots.iter().enumerate().any(|(i,r)|
            !image.matches_root_origin(i, r.name.path().as_os_str().as_bytes(), r.component_limit, r.observed.fields())) {
            return Err(invalid("new image differs from the held original root selection"));
        }
        Ok(())
    }
}

fn leaf_observation(mounts: &Mounts, r: &Root, named: &File, image: Option<&FreshGenesis<'_>>, i: usize)
    -> io::Result<(Vec<u64>, bool)> {
    match image {
        Some(g) => mounts.pending_chain(&r.name, named, g, i),
        None => mounts.unclaimed_chain(&r.name, named).map(|chain| (chain, false)),
    }
}

fn sample_roots(roots: &[Root], uid: u32, image: Option<(&FreshGenesis<'_>, RegisterName)>) -> io::Result<Vec<RootSample>> {
    let mounts = Mounts::current()?;
    roots.iter().enumerate().map(|(i, r)| sample_one(&mounts, r, uid, image, i)).collect()
}

fn sample_one(mounts: &Mounts, r: &Root, uid: u32, image: Option<(&FreshGenesis<'_>, RegisterName)>, i: usize) -> io::Result<RootSample> {
    let named = r.checked_name(uid)?;
    let stamp = DirectoryStamp::on(&named)?;
    let (chain, labelled) = leaf_observation(mounts, r, &named, image.map(|(g, _)| g), i)?;
    match (i, image) {
        (0, Some((_, name))) => read_only_register(named, name)?,
        _ => read_empty(named)?,
    }
    Ok(RootSample { stamp, mounts: chain, labelled })
}

fn finish_roots(roots: &[Root], uid: u32, image: Option<&FreshGenesis<'_>>, samples: &[RootSample]) -> io::Result<()> {
    if samples.len() != roots.len() { return Err(invalid("incomplete storage root observation")); }
    let mounts = Mounts::current()?;
    for (i, (r, before)) in roots.iter().zip(samples).enumerate() {
        finish_one(&mounts, r, uid, image, i, before)?;
    }
    Ok(())
}

fn finish_one(mounts: &Mounts, r: &Root, uid: u32, image: Option<&FreshGenesis<'_>>, i: usize, before: &RootSample) -> io::Result<()> {
    let named = r.checked_name(uid)?;
    let (chain, labelled) = leaf_observation(mounts, r, &named, image, i)?;
    if chain != before.mounts || labelled != before.labelled
        || DirectoryStamp::on(&named)? != before.stamp || DirectoryStamp::on(&r.dir)? != before.stamp {
        return Err(invalid("storage directory changed during observation"));
    }
    Ok(())
}

// A newly opened directory description starts at offset zero on each scan;
// never advance the original locked description. RawDir preserves getdents
// errors (including ENOENT on removal); only its real end counts as EOF.
fn read_empty(dir: File) -> io::Result<()> { read_directory(dir, None) }
pub(super) fn read_only_register(dir: File, name: RegisterName) -> io::Result<()> { read_directory(dir, Some(name)) }

fn read_directory(dir: File, expected: Option<RegisterName>) -> io::Result<()> {
    let mut buffer = [MaybeUninit::<u8>::uninit(); 4096];
    let mut stream = RawDir::new(dir, &mut buffer);
    let mut dot = false;
    let mut parent = false;
    let mut found = false;
    while let Some(entry) = stream.next() {
        let entry = entry?;
        match entry.file_name().to_bytes() {
            b"." if !dot => dot = true,
            b".." if !parent => parent = true,
            entry if !found && expected.is_some_and(|name| name.entry().to_bytes() == entry) => found = true,
            _ => return Err(invalid("unexpected entry or inconsistent storage directory stream")),
        }
    }
    if found != expected.is_some() { return Err(invalid("the required original registration name is absent")); }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/maintenance/root_set.rs"]
mod tests;
