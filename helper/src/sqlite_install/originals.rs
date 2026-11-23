//! All selected origins stay pinned from the operator's preview to SQLite close.
//! No state is read from legacy formats, and no path comes from an untrusted SQL row.
use super::{MARKER, bad, request::Request, unsupported};
use crate::{
    maintenance::HostMounts,
    pinned::{DiscoveryAdmission, Entity, Kind, Mount, Pinned, same_mount_leaf},
};
use rustix::fs::{
    AtFlags, FlockOperation, Mode, OFlags, RawDir, ResolveFlags, StatVfsMountFlags, fgetxattr,
    flock, fstatvfs, openat, openat2, statat,
};
use std::{
    ffi::OsStr,
    fs::File,
    io,
    mem::MaybeUninit,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
};

pub(super) struct Originals {
    pub(super) selected: Vec<Pinned>,
    host: File,
}
pub(super) struct Group {
    originals: Originals, // root-owned O_PATH origins permanently retained
    dirs: Vec<File>,
    limits: Vec<u8>,
    uid: u32,
}
fn location(f: &File) -> io::Result<(Entity, Mount)> {
    Ok((
        Entity::of(&f.metadata()?)?,
        Mount::on(f)?.ok_or_else(|| unsupported("live mount identity unavailable"))?,
    ))
}

/// These borrows never create, duplicate or convert a lock. A Group builds the
/// view from one of its exclusively held roots; an independent S-only cut owns
/// its own exclusive, separately opened original SYSTEM OFD for its whole life.
#[derive(Clone, Copy)]
pub(super) struct HeldRoot<'g> {
    pin: &'g Pinned,
    dir: &'g File,
    uid: u32,
    limit: u8,
    needs_write: bool,
}
impl<'g> HeldRoot<'g> {
    pub(super) fn dir(self) -> &'g File {
        self.dir
    }
    pub(super) fn uid(self) -> u32 {
        self.uid
    }
    pub(super) fn path(self) -> &'g std::path::Path {
        self.pin.path()
    }
    pub(super) fn byte_limit(self) -> u8 {
        self.limit
    }
    pub(super) fn local_only(self) -> bool {
        self.pin.local_scope().is_some()
    }
    pub(super) fn require(self) -> io::Result<()> {
        self.pin.admit_held(self.dir)?;
        let m = self.dir.metadata()?;
        if location(self.dir)? != (self.pin.entity(), self.pin.mount()?)
            || location(&self.pin.storage_directory()?)? != location(self.dir)?
            || m.uid() != self.uid
            || m.mode() & 0o7777 != 0o700
            || m.nlink() == 0
            || !m.is_dir()
            || helper_linux_evidence::portable_component_limit(self.dir)? != self.limit
            || usize::from(self.limit) < MARKER.len()
            || self.needs_write
                && fstatvfs(self.dir)?
                    .f_flag
                    .contains(StatVfsMountFlags::RDONLY)
        {
            return Err(bad(
                "changed, read-only, short-name or unprotected original root",
            ));
        }
        Ok(())
    }
    pub(super) fn controls(self, allowed: &[&[u8]]) -> io::Result<Vec<bool>> {
        let mut seen = vec![false; allowed.len()];
        let again = self.pin.storage_directory()?;
        if location(&again)? != location(self.dir)? {
            return Err(bad("directory changed before inventory"));
        }
        let mut buf = [MaybeUninit::uninit(); 4096];
        let mut dir = RawDir::new(&again, &mut buf);
        while let Some(e) = dir.next() {
            let e = e?;
            let name = e.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            let j = allowed
                .iter()
                .position(|s| *s == name)
                .ok_or_else(|| bad("original root holds an unknown entry; no takeover"))?;
            if std::mem::replace(&mut seen[j], true) {
                return Err(bad("duplicated root control name"));
            }
        }
        self.require()?;
        Ok(seen)
    }
}

/// Separate capability for one established SYSTEM. It cannot be
/// substituted for an initial whole-group grant or constructed from SQL paths.
pub(super) struct SelectedSystem {
    pin: Pinned,
    host: File,
    uid: u32,
    gid: u32,
}
#[derive(Clone, Copy)]
enum ExclusiveLease {
    Owned,
    Lent,
}
/// The original SYSTEM with an acquired or independently proven exclusive lease.
pub(super) struct LockedSystem {
    selected: SelectedSystem,
    dir: File,
    limit: u8,
    lease: ExclusiveLease,
}
impl SelectedSystem {
    #[cfg(test)]
    pub(super) fn from_host(
        path: &std::path::Path,
        host: File,
        uid: u32,
        gid: u32,
    ) -> io::Result<Self> {
        let pin = Pinned::open(path, Kind::StorageRoot)?;
        Self::from_origin(pin, host, uid, gid)
    }
    // For the already authenticated, permanently lowered host worker. Its
    // O_PATH was matched against the parent's still-held original before use.
    pub(super) fn from_origin(pin: Pinned, host: File, uid: u32, gid: u32) -> io::Result<Self> {
        super::request::database_path(pin.path(), super::engine::path_limit()?)?;
        let m = pin.metadata()?;
        if uid == 0
            || gid == 0
            || uid == u32::MAX
            || gid == u32::MAX
            || !m.is_dir()
            || m.uid() != uid
            || m.mode() & 0o7777 != 0o700
            || m.nlink() == 0
        {
            return Err(bad(
                "expected the configured storage worker's private original SYSTEM",
            ));
        }
        Ok(Self {
            pin,
            host,
            uid,
            gid,
        })
    }
    pub(super) fn lock(self) -> io::Result<LockedSystem> {
        crate::maintenance::diagnostic_worker(self.uid, self.gid)?;
        self.take_original()
    }
    fn take_original(self) -> io::Result<LockedSystem> {
        let dir = self.pin.storage_directory()?;
        flock(&dir, FlockOperation::NonBlockingLockExclusive)?;
        self.with_descriptor(dir, ExclusiveLease::Owned)
    }
    /// The dedicated host issuer already owns this OFD and retains it through
    /// the action. Do NOT open a second EX or convert the shared description.
    pub(super) fn hold_lent_ex(self, dir: File) -> io::Result<LockedSystem> {
        crate::maintenance::diagnostic_worker(self.uid, self.gid)?;
        self.with_descriptor(dir, ExclusiveLease::Lent)
    }
    fn with_descriptor(self, dir: File, lease: ExclusiveLease) -> io::Result<LockedSystem> {
        self.pin.admit_held(&dir)?;
        let limit = helper_linux_evidence::portable_component_limit(&dir)?;
        let locked = LockedSystem {
            selected: self,
            dir,
            limit,
            lease,
        };
        locked.require()?;
        Ok(locked)
    }
}
impl LockedSystem {
    pub(super) fn source(&self) -> HeldRoot<'_> {
        HeldRoot {
            pin: &self.selected.pin,
            dir: &self.dir,
            uid: self.selected.uid,
            limit: self.limit,
            needs_write: false,
        }
    }
    pub(super) fn require(&self) -> io::Result<()> {
        self.source().require()?;
        let id = (
            self.selected.pin.entity(),
            self.selected.pin.require_same_named_mount()?.id,
        );
        if matches!(self.lease, ExclusiveLease::Lent) {
            super::pair::require_lent_ex(&self.dir, id)?;
        }
        if self.selected.pin.local_scope().is_some() {
            guarded_named_backing(
                &self.selected.pin,
                &self.dir,
                &self.selected.host,
                self.selected.uid,
            )?;
        } else {
            ancestry(
                &self.selected.pin,
                &self.dir,
                &HostMounts::current()?,
                &self.selected.host,
                self.selected.uid,
            )?;
        }
        Ok(())
    }
    /// Read-only H04 uses the same actual offline lease, not any writer gate.
    /// Its selected S must already have been pinned without automounting.
    pub(super) fn retirement_host(&self) -> io::Result<(&File, &Pinned)> {
        if !self.selected.pin.passive_mount_observation() {
            return Err(bad(
                "retirement inventory needs an already mounted original SYSTEM and its offline EX lease",
            ));
        }
        self.require()?;
        Ok((&self.selected.host, &self.selected.pin))
    }
    #[cfg(test)]
    pub(super) fn from_fixture(path: &std::path::Path) -> io::Result<Self> {
        SelectedSystem::from_host(
            path,
            File::open("/")?,
            rustix::process::getuid().as_raw(),
            rustix::process::getgid().as_raw(),
        )?
        .take_original()
    }
    #[cfg(test)]
    pub(super) fn from_passive_fixture(path: &std::path::Path) -> io::Result<Self> {
        let pin = Pinned::already_mounted(path, Kind::StorageRoot)?;
        SelectedSystem::from_origin(
            pin,
            File::open("/")?,
            rustix::process::getuid().as_raw(),
            rustix::process::getgid().as_raw(),
        )?
        .take_original()
    }
}

impl Originals {
    pub(super) fn require_host(&self, fd: &File) -> io::Result<()> {
        if location(&self.host)? != location(fd)? {
            return Err(bad("host root changed during operator approval"));
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn pin(request: &Request, host: File) -> io::Result<Self> {
        Self::pin_with(request, host, Pinned::open)
    }
    // Before displaying the real root list, use the held host root and verify
    // already mounted originals without consulting an on-disk declaration.
    pub(super) fn pin_passive(request: &Request, host: File) -> io::Result<Self> {
        Self::pin_with(request, host, Pinned::already_mounted)
    }
    fn pin_with(
        request: &Request,
        host: File,
        open: impl Fn(&std::path::Path, Kind) -> io::Result<Pinned>,
    ) -> io::Result<Self> {
        let selected = request
            .paths()
            .iter()
            .map(|p| open(p, Kind::StorageRoot))
            .collect::<io::Result<Vec<_>>>()?;
        for (i, r) in selected.iter().enumerate() {
            if selected[..i].iter().any(|a| {
                a.entity() == r.entity()
                    || a.entity_is_visible_ancestor_of(r)
                    || r.entity_is_visible_ancestor_of(a)
            }) {
                return Err(bad("duplicate or nested original storage directories"));
            }
            let m = r.metadata()?;
            if !m.is_dir()
                || m.uid() != request.uid()
                || m.mode() & 0o7777 != 0o700
                || m.nlink() == 0
            {
                return Err(bad(
                    "a storage root must be a private 0700 directory of the selected storage worker",
                ));
            }
        }
        Ok(Self { selected, host })
    }

    /// A separate participant always obtains a fresh open description for each
    /// directory. S is locked exactly once as part of this list, never upgraded.
    pub(super) fn lock(self, uid: u32) -> io::Result<Group> {
        let dirs = self
            .selected
            .iter()
            .map(Pinned::storage_directory)
            .collect::<io::Result<Vec<_>>>()?;
        let mut order: Vec<_> = (0..dirs.len()).collect();
        order.sort_unstable_by_key(|&i| {
            let e = self.selected[i].entity();
            (e.dev, e.ino)
        });
        for i in order {
            flock(&dirs[i], FlockOperation::NonBlockingLockExclusive)?;
        }
        let limits = dirs
            .iter()
            .map(helper_linux_evidence::portable_component_limit)
            .collect::<io::Result<Vec<_>>>()?;
        let g = Group {
            originals: self,
            dirs,
            limits,
            uid,
        };
        g.recheck()?;
        Ok(g)
    }
}

impl Group {
    pub(super) fn matches(&self, request: &Request) -> bool {
        self.originals
            .selected
            .iter()
            .map(super::super::pinned::Pinned::path)
            .eq(request.paths().iter().map(std::path::PathBuf::as_path))
    }
    fn root(&self, i: usize) -> HeldRoot<'_> {
        HeldRoot {
            pin: &self.originals.selected[i],
            dir: &self.dirs[i],
            uid: self.uid,
            limit: self.limits[i],
            needs_write: true,
        }
    }
    pub(super) fn system_source(&self) -> HeldRoot<'_> {
        self.root(0)
    }
    pub(super) fn system(&self) -> &File {
        &self.dirs[0]
    }
    pub(super) fn dirs(&self) -> &[File] {
        &self.dirs
    }
    pub(super) fn limits(&self) -> &[u8] {
        &self.limits
    }
    pub(super) fn uid(&self) -> u32 {
        self.uid
    }
    pub(super) fn scan_names(&self, i: usize, allowed: &[&[u8]]) -> io::Result<()> {
        if self.controls(i, allowed)?.contains(&false) {
            return Err(bad("missing expected root control name"));
        }
        Ok(())
    }
    // Presence and raw spelling come from a complete independent directory
    // stream, not case-insensitive name lookup or an ENOENT from an old parent.
    pub(super) fn controls(&self, i: usize, allowed: &[&[u8]]) -> io::Result<Vec<bool>> {
        self.root(i).controls(allowed)
    }
    fn check_root(&self, i: usize) -> io::Result<()> {
        self.root(i).require()
    }
    pub(super) fn recheck(&self) -> io::Result<()> {
        let table = HostMounts::current()?;
        let mut sb = Vec::with_capacity(self.dirs.len());
        for (i, f) in self.dirs.iter().enumerate() {
            self.check_root(i)?;
            sb.push(ancestry(
                &self.originals.selected[i],
                f,
                &table,
                &self.originals.host,
                self.uid,
            )?);
        }
        // Host administrator also explicitly attests that the selected original R
        // has separate backing. Distinct mount IDs, paths or device numbers ALONE
        // do not establish physical independence or rule out stacked re-exports.
        for i in (0..self.dirs.len()).filter(|&i| i != 1) {
            let a = &self.originals.selected[i];
            let r = &self.originals.selected[1];
            if sb[i] == sb[1] || a.mount()?.id == r.mount()?.id || a.entity().dev == r.entity().dev
            {
                return Err(unsupported(
                    "recycle root does not have a separately attributable filesystem",
                ));
            }
        }
        Ok(())
    }
}

// This is only a negative takeover check for a known historical mark. Neither
// the format nor any SQLite receipt is issued from xattrs. Never read its value.
// For existing operator-supplied directories, lack of observable provenance
// (including an unavailable xattr namespace) cannot authorize first enrollment.
fn reject_old_claim(fd: &File) -> io::Result<()> {
    let dir = File::from(openat(
        fd,
        c".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?);
    if location(&dir)? != location(fd)? {
        return Err(bad("directory changed before the legacy occupancy check"));
    }
    require_no_old_attribute(&dir)
}
fn require_no_old_attribute(dir: &File) -> io::Result<()> {
    let mut empty = [0u8; 0];
    match fgetxattr(dir, c"user.zxs.volume", &mut empty[..]) {
        Err(rustix::io::Errno::NODATA) => Ok(()),
        Ok(_) | Err(rustix::io::Errno::RANGE) => Err(bad(
            "root or ancestor has an existing volume attribute; no takeover",
        )),
        Err(rustix::io::Errno::OPNOTSUPP) => Err(unsupported(
            "cannot establish prior ownership on this mount",
        )),
        Err(e) => Err(e.into()),
    }
}
/// Inspect only a presently held media origin or an existing protected parent.
/// The backing number is the kernel mount-table superblock observation, not
/// a durable UUID or an assertion that the physical disks are independent.
pub(super) fn named_backing(
    name: &Pinned,
    leaf: &File,
    table: &HostMounts,
    host: &File,
    uid: u32,
) -> io::Result<u64> {
    let expected = (name.entity(), name.require_same_named_mount()?);
    if location(leaf)? != expected {
        return Err(bad("substituted original volume or parent"));
    }
    let backing = ancestry(name, leaf, table, host, uid)?;
    if location(leaf)? != expected || name.require_same_named_mount()? != expected.1 {
        return Err(bad("original volume or protected parent changed"));
    }
    Ok(backing)
}
/// Inventory keeps the existing whole-mount, original ancestry, permission
/// and old-claim checks, but does them top-down on positively local prefixes.
pub(super) fn guarded_named_backing(
    name: &Pinned,
    leaf: &File,
    host: &File,
    uid: u32,
) -> io::Result<u64> {
    let scope = name
        .local_scope()
        .ok_or_else(|| bad("need the original local mount scope"))?;
    scope.admit(host)?;
    scope.admit(leaf)?;
    let root = location(host)?;
    let original = location(leaf)?;
    if original != (name.entity(), name.mount()?) {
        return Err(bad("substituted admitted volume"));
    }
    let mut previous = None;
    let mut backing = None;
    let mounted = name.same_local_chain(|f, i| {
        let (entity, mount) = location(f)?;
        let (parent, device) = scope.full_local_mount(mount.id)?;
        if i == 0 && ((entity, mount) != root || mount.at_root != Some(true))
            || previous
                .is_some_and(|p| p != mount.id && (mount.at_root != Some(true) || parent != p))
        {
            return Err(unsupported(
                "cannot attribute this original local directory chain",
            ));
        }
        previous = Some(mount.id);
        let last = i + 1 == name.visible_chain().len();
        if last && (entity, mount) != original {
            return Err(bad("the original locally admitted leaf changed"));
        }
        let m = f.metadata()?;
        if !m.is_dir()
            || m.nlink() == 0
            || !(m.uid() == 0 || m.uid() == uid)
            || m.mode() & 0o022 != 0 && m.mode() & 0o1000 == 0
        {
            return Err(bad("unprotected original local directory"));
        }
        // "." stays on this positively admitted directory. Reading an xattr
        // predicate needs a real description, not a re-open of its path.
        let rd = File::from(openat2(
            f,
            c".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_XDEV | ResolveFlags::NO_SYMLINKS,
        )?);
        scope.admit(&rd)?;
        if location(&rd)? != (entity, mount) {
            return Err(bad("local directory changed before occupancy check"));
        }
        require_no_old_attribute(&rd)?;
        if last {
            backing = Some(device);
        } else {
            match same_mount_leaf(f, OsStr::from_bytes(MARKER)) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
                Ok(_) => return Err(bad("reserved claim present on an original ancestor")),
            }
        }
        Ok(())
    })?;
    scope.admit(leaf)?;
    if mounted != original.1 || location(leaf)? != original {
        return Err(bad("original local volume changed"));
    }
    backing.ok_or_else(|| bad("missing local filesystem origin"))
}

fn ancestry(
    name: &Pinned,
    leaf: &File,
    table: &HostMounts,
    host: &File,
    uid: u32,
) -> io::Result<u64> {
    let host = location(host)?;
    let mut seen = name.visible_chain().iter().rev().peekable();
    let mut up = None;
    let mut child: Option<(Mount, u64)> = None;
    let mut sb = None;
    while let Some(expected) = seen.next() {
        let fd = up.as_ref().unwrap_or(leaf);
        let (id, m) = location(fd)?;
        let md = fd.metadata()?;
        if id != *expected
            || !md.is_dir()
            || md.nlink() == 0
            || !(md.uid() == 0 || md.uid() == uid)
            || md.mode() & 0o022 != 0 && md.mode() & 0o1000 == 0
        {
            return Err(bad("unprotected or changed directory ancestry"));
        }
        reject_old_claim(fd)?;
        let (parent, device) = table.full_mount(m.id)?;
        if let Some((below, expected_parent)) = child {
            if m.id != below.id && (below.at_root != Some(true) || expected_parent != m.id) {
                return Err(unsupported("cannot attribute this mount crossing"));
            }
            match statat(
                fd,
                OsStr::from_bytes(MARKER),
                AtFlags::SYMLINK_NOFOLLOW | AtFlags::NO_AUTOMOUNT,
            ) {
                Err(rustix::io::Errno::NOENT) => {}
                Err(e) => return Err(e.into()),
                Ok(_) => return Err(bad("reserved claim present on a selected ancestor")),
            }
        } else {
            sb = Some(device);
        }
        if seen.peek().is_none() {
            if (id, m) != host || m.at_root != Some(true) {
                return Err(unsupported(
                    "selected ancestry did not reach independently held host root",
                ));
            }
            break;
        }
        child = Some((m, parent));
        up = Some(File::from(openat(
            fd,
            c"..",
            OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?));
    }
    sb.ok_or_else(|| bad("missing filesystem origin"))
}
