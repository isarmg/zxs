//! Kernel observations of explicitly selected directories. Not a pool or role admission.
use super::{Entity, Kind, Mount, Pinned};
use rustix::fs::{StatVfsMountFlags, fstatvfs};
use std::{ffi::OsString, fmt::{self, Display}, fs::File, io, os::unix::ffi::OsStrExt, path::{Path, PathBuf}, time::{SystemTime, UNIX_EPOCH}};

const MAX_DATA: usize = 62;

#[derive(Debug)]
pub(crate) struct Selection { system: PathBuf, recycle: PathBuf, data: Vec<PathBuf> }
impl Selection {
    pub(crate) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self, &'static str> {
        let (mut system, mut recycle, mut data) = (None, None, Vec::new());
        while let Some(flag) = args.next() {
            let path = PathBuf::from(args.next().ok_or("expected a directory after each role")?);
            if Kind::StorageRoot.check_path(path.as_os_str().as_bytes()).is_err() {
                return Err("expected a bounded, canonical absolute storage directory other than /");
            }
            if flag == "--system" && system.is_none() { system = Some(path); }
            else if flag == "--recycle" && recycle.is_none() { recycle = Some(path); }
            else if flag == "--data" && data.len() < MAX_DATA { data.push(path); }
            else { return Err("unknown or duplicate role option, or more than 62 data directories"); }
        }
        if data.is_empty() { return Err("supply at least one --data directory"); }
        Ok(Self { system: system.ok_or("missing --system directory")?,
                  recycle: recycle.ok_or("missing --recycle directory")?, data })
    }

    pub(crate) fn paths(&self) -> impl Iterator<Item = (Role, &Path)> {
        [(Role::System, self.system.as_path()), (Role::Recycle, self.recycle.as_path())].into_iter()
            .chain(self.data.iter().enumerate().map(|(i, p)| (Role::Data(i + 1), p.as_path())))
    }

    pub(crate) fn observe(&self) -> Report {
        let started = timestamp();
        let rows = self.paths().map(|(role, p)| (role, observe(p))).collect();
        Report { started, ended: timestamp(), rows }
    }
}

/// The local panel has no path-taking operation. Capture only the explicit
/// foreground selection, before the listener, without waking automounts or
/// taking capacity samples. Never adopt a later file at a missing/changed name.
pub(crate) struct WebSelection {
    roots: Vec<(Role, Result<WebRoot, Reason>)>,
}
struct WebRoot { pin: Pinned, mount: Mount }
impl WebSelection {
    pub(crate) fn pin(selection: Selection) -> Self {
        Self { roots: selection.paths().map(|(role, path)| (role, WebRoot::pin(path))).collect() }
    }

    pub(crate) fn observe(&self) -> Report {
        let started = timestamp();
        let rows = self.roots.iter().map(|(role, root)| (*role, match root {
            Ok(root) => root.sample(),
            Err(why) => Err(*why),
        })).collect();
        Report { started, ended: timestamp(), rows }
    }
}
impl WebRoot {
    fn pin(path: &Path) -> Result<Self, Reason> {
        let pin = Pinned::already_mounted(path, Kind::StorageRoot).map_err(Reason::opening_directory)?;
        let mount = observed_mount(&pin.fd)?;
        let root = Self { pin, mount };
        root.require()?;
        Ok(root)
    }

    fn require(&self) -> Result<(), Reason> {
        // The observer can only sample its original O_PATH handle. The
        // independently resolved name, all visible parents and the mount
        // must match both before and after; the recheck is never a fallback.
        let current = self.pin.require_same_named_mount().map_err(Reason::recheck_directory)?;
        if current == self.mount { Ok(()) } else { Err(Reason::Changed) }
    }

    fn sample(&self) -> Result<Observed, Reason> {
        self.require()?;
        let space = Space::read(&self.pin.fd);
        self.require()?;
        Ok(Observed { entity: self.pin.entity(), mount: Ok(Mounted { mount: self.mount, space }) })
    }
}

fn timestamp() -> Option<u128> {
    SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis())
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum Reason { Unavailable, Denied, Unsupported, Invalid, Changed, NotReported, Io(Option<i32>) }
impl From<io::Error> for Reason {
    fn from(value: io::Error) -> Self {
        use io::ErrorKind::*;
        if value.raw_os_error() == Some(rustix::io::Errno::NOSYS.raw_os_error()) { return Self::Unsupported; }
        match value.kind() {
            NotFound => Self::Unavailable, PermissionDenied => Self::Denied, Unsupported => Self::Unsupported,
            _ => Self::Io(value.raw_os_error()),
        }
    }
}
impl Reason {
    fn opening_directory(e: io::Error) -> Self {
        if matches!(e.kind(), io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData | io::ErrorKind::NotADirectory) {
            Self::Invalid
        } else { e.into() }
    }
    fn recheck_directory(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData |
                io::ErrorKind::NotADirectory => Self::Changed,
            _ => e.into(),
        }
    }
}
impl Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => f.write_str("not-found"), Self::Denied => f.write_str("permission-denied"),
            Self::Unsupported => f.write_str("unsupported"), Self::Invalid => f.write_str("not-an-admitted-directory"),
            Self::Changed => f.write_str("changed"), Self::NotReported => f.write_str("not-reported"),
            Self::Io(Some(n)) => write!(f,"io-error({n})"), Self::Io(None) => f.write_str("io-error"),
        }
    }
}

fn observed_mount(fd: &File) -> Result<Mount, Reason> {
    Mount::on(fd).map_err(Reason::from)?.ok_or(Reason::NotReported)
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) struct Capacity { pub(crate) total: u128, pub(crate) free: u128, pub(crate) available: u128 }
impl Capacity {
    fn counts(unit: u64, total: u64, free: u64, available: u64) -> Option<Self> {
        if unit == 0 || total == 0 || free > total || available > free { return None; }
        let bytes = |n: u64| u128::from(n) * u128::from(unit);
        Some(Self { total: bytes(total), free: bytes(free), available: bytes(available) })
    }
}
#[derive(Debug)]
pub(crate) struct Space { pub(crate) readonly_flag: bool, pub(crate) capacity: Option<Capacity> }
impl Space {
    fn read(fd: &File) -> Result<Self, Reason> {
        let v = fstatvfs(fd).map_err(|e| Reason::from(io::Error::from(e)))?;
        Ok(Self { readonly_flag: v.f_flag.contains(StatVfsMountFlags::RDONLY),
                  capacity: Capacity::counts(v.f_frsize, v.f_blocks, v.f_bfree, v.f_bavail) })
    }
}
#[derive(Debug)]
pub(crate) struct Mounted { pub(crate) mount: Mount, pub(crate) space: Result<Space, Reason> }
#[derive(Debug)]
pub(crate) struct Observed { pub(crate) entity: Entity, pub(crate) mount: Result<Mounted, Reason> }

// CLI as well as Web observes only the already-mounted directory. Keep its
// O_PATH descriptor through attribution; do not take a pool or volume lock.
struct HeldSample { root: Pinned, mount: Result<Mounted, Reason> }
impl HeldSample {
    fn take(path: &Path) -> Result<Self, Reason> {
        let root = Pinned::already_mounted(path, Kind::StorageRoot).map_err(Reason::opening_directory)?;
        // Per-directory filesystem accounting can differ between subvolumes.
        let mount = observed_mount(&root.fd).map(|mount| Mounted { mount, space: Space::read(&root.fd) });
        Ok(Self { root, mount })
    }
    fn finish(self) -> Result<Observed, Reason> {
        let now = Pinned::already_mounted(&self.root.path, Kind::StorageRoot).map_err(Reason::recheck_directory)?;
        if self.root.chain != now.chain { return Err(Reason::Changed); }
        let mount = match self.mount {
            Ok(held) => match observed_mount(&now.fd) {
                Ok(m) if held.mount.id == m.id => Ok(held),
                // Bind mounts of the same inode can still have different per-mount flags.
                Ok(_) => return Err(Reason::Changed),
                Err(why) => Err(why),
            },
            Err(why) => Err(why),
        };
        Ok(Observed { entity: self.root.entity(), mount })
    }
}

fn observe(path: &Path) -> Result<Observed, Reason> { HeldSample::take(path)?.finish() }

#[derive(Debug, Copy, Clone)]
pub(crate) enum Role { System, Recycle, Data(usize) }
impl Display for Role {
    fn fmt(&self, f:&mut fmt::Formatter<'_>) -> fmt::Result {
        match self { Self::System => f.write_str("system"), Self::Recycle => f.write_str("recycle"),
                     Self::Data(i) => write!(f,"data[{i}]") }
    }
}
#[derive(Debug)]
pub(crate) struct Report {
    pub(crate) started: Option<u128>,
    pub(crate) ended: Option<u128>,
    pub(crate) rows: Vec<(Role, Result<Observed, Reason>)>,
}
impl Report {
    pub(crate) fn all_sampled(&self) -> bool {
        self.rows.iter().all(|(_,r)| matches!(r,Ok(Observed { mount: Ok(Mounted { space: Ok(Space { capacity: Some(_), .. }), .. }), .. })))
    }
}
impl Display for Report {
    fn fmt(&self, f:&mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "source=linux-statx+fstatvfs roles=operator-selected registration=not-checked filesystem_uuid=not-observed business_health=not-observed")?;
        match (self.started, self.ended) {
            (Some(a), Some(b)) if b >= a => write!(f, " sample_unix_ms={a}..{b}")?,
            _ => f.write_str(" sample_unix_ms=unavailable")?,
        }
        for (role, row) in &self.rows {
            write!(f,"\nrole={role} ")?;
            let o = match row { Ok(o) => o, Err(why) => { write!(f,"path=unverified reason={why}")?; continue; } };
            write!(f, "path=observed dev={} ino={} ", o.entity.dev, o.entity.ino)?;
            let Mounted { mount: m, space: s } = match &o.mount { Ok(mounted) => mounted, Err(why) => {
                write!(f,"mount_id=unknown reason={why} mount_ro_flag=unattributed capacity=unattributed")?; continue;
            }};
            write!(f,"mount_id={} mount_root={} ",m.id, match m.at_root { Some(true)=>"yes", Some(false)=>"no", None=>"not-reported" })?;
            match s {
                Err(why) => write!(f, "mount_ro_flag=unknown capacity=unknown reason={why}")?,
                Ok(s) => {
                    write!(f,"mount_ro_flag={} ",if s.readonly_flag { "set" } else { "clear" })?;
                    match s.capacity {
                        Some(c) => write!(f,"total_bytes={} free_bytes={} available_bytes={}",c.total,c.free,c.available)?,
                        None => f.write_str("capacity=unknown reason=unusable-kernel-counts")?,
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn online_storage_observers_do_not_require_an_offline_system_lease() {
        use rustix::fs::{FlockOperation, flock};

        let area = tempfile::tempdir().unwrap();
        let system = area.path().join("system");
        let recycle = area.path().join("recycle");
        let data = area.path().join("data");
        for dir in [&system, &recycle, &data] {
            std::fs::create_dir(dir).unwrap();
        }
        let selection = || Selection { system: system.clone(), recycle: recycle.clone(), data: vec![data.clone()] };
        let owner = File::open(&system).unwrap();
        // Simulate an online lifecycle hold and then a separate exclusive
        // holder, using only this temporary directory, not a deployed S.
        for held in [FlockOperation::NonBlockingLockShared, FlockOperation::NonBlockingLockExclusive] {
            flock(&owner, held).unwrap();
            let original = HeldSample::take(&system).unwrap();
            assert!(original.root.passive_mount_observation());
            let cli = selection().observe();
            let web = WebSelection::pin(selection()).observe();
            for report in [cli, web] {
                assert_eq!(report.rows.len(), 3);
                assert!(report.rows.iter().all(|(_, v)| v.as_ref().is_ok_and(|r| r.mount.is_ok())));
            }
            let other = File::open(&system).unwrap();
            assert_eq!(flock(&other, FlockOperation::NonBlockingLockExclusive), Err(rustix::io::Errno::WOULDBLOCK));
            flock(&owner, FlockOperation::Unlock).unwrap();
        }
        assert_eq!(std::fs::read_dir(&system).unwrap().count(), 0);
    }

    #[test]
    fn role_selection_is_bounded_and_preserves_non_utf8_unix_names() {
        let bytes = OsString::from_vec(vec![b'/',0xfe]);
        let base = [OsString::from("--recycle"),"/r".into(),"--system".into(),"/s".into()];
        let a = |n| base.clone().into_iter().chain((0..n).flat_map(|_| [OsString::from("--data"),bytes.clone()]));
        let selected = Selection::parse(a(62)).unwrap();
        assert_eq!(selected.data[61].as_os_str(), bytes.as_os_str());
        assert!(Selection::parse(a(63)).is_err());
        assert!(Selection::parse(a(0)).is_err());
        for wrong in ["/", "relative", "/x/..", "/a//b", "/x/."] {
            assert!(Selection::parse(base.clone().into_iter().chain(["--data".into(),wrong.into()])).is_err());
        }
        assert!(Selection::parse(a(1).chain(["--system".into(),"/other".into()])).is_err());
        assert!(Selection::parse(a(1).chain(["--arbitrary".into(),"/other".into()])).is_err());
        assert!(Selection::parse(["--data".into(),"/d".into()].into_iter()).is_err());
    }

    #[test]
    fn rejected_kernel_sample_does_not_reclassify_an_observed_directory() {
        let errno = rustix::io::Errno::INVAL.raw_os_error();
        assert_eq!(Reason::from(io::Error::from_raw_os_error(errno)),Reason::Io(Some(errno)));
        assert_eq!(Reason::opening_directory(io::Error::from_raw_os_error(errno)),Reason::Invalid);
    }

    #[test]
    fn replacing_only_a_visible_parent_invalidates_the_mount_sample() {
        let area = tempfile::tempdir().unwrap();
        let parent = area.path().join("parent");
        let old = area.path().join("saved-parent");
        let selected = parent.join("root");
        std::fs::create_dir(&parent).unwrap(); std::fs::create_dir(&selected).unwrap();
        let held = HeldSample::take(&selected).unwrap();
        std::fs::rename(&parent, &old).unwrap(); std::fs::create_dir(&parent).unwrap();
        std::fs::rename(old.join("root"), &selected).unwrap();
        assert_eq!(Pinned::open(&selected, Kind::Directory).unwrap().entity(),held.root.entity());
        assert!(matches!(held.finish(),Err(Reason::Changed)));
    }

    #[test]
    fn a_storage_sample_and_its_final_reopen_keep_the_maintenance_depth_limit() {
        let area = tempfile::tempdir().unwrap();
        let mut dir = area.path().to_owned();
        let base = dir.components().count() - 1; // the leading slash is not a component
        assert!(base < 128, "fixture needs a private absolute path below 128 components");
        for depth in base + 1..=256 {
            dir.push("n"); std::fs::create_dir(&dir).unwrap();
            if [128, 129, 256].contains(&depth) {
                let sample = HeldSample::take(&dir).unwrap();
                assert_eq!(sample.finish().unwrap().entity, Entity::of(&std::fs::metadata(&dir).unwrap()).unwrap());
                assert_eq!(Pinned::open(&dir, Kind::Directory).is_ok(), depth <= 128);
            }
        }
        dir.push("not-opened");
        assert!(matches!(HeldSample::take(&dir), Err(Reason::Invalid)));
    }

    #[test]
    fn real_zero_availability_is_distinct_from_invalid_counters_and_product_does_not_wrap() {
        assert_eq!(Capacity::counts(4096,8,2,0),Some(Capacity { total:32768, free:8192, available:0 }));
        assert_eq!(Capacity::counts(0,1,1,1),None);
        assert_eq!(Capacity::counts(4096,0,0,0),None);
        assert_eq!(Capacity::counts(2,1,2,1),None);
        assert_eq!(Capacity::counts(2,3,1,2),None);
        let cap = Capacity::counts(u64::MAX,u64::MAX,2,1).unwrap();
        assert_eq!(cap.total, u128::from(u64::MAX).pow(2));
    }
}
