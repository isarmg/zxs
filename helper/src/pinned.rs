//! Resolve inputs using held directories, without adopting a replacement at the same spelling.
pub(crate) mod helper_elf;
pub(crate) mod storage_status;
pub(crate) use crate::path_rules::Kind;

use crate::maintenance::DiscoveryView;
use rustix::fs::{
    AtFlags, CWD, Mode, OFlags, PROC_SUPER_MAGIC, ResolveFlags, StatxAttributes, StatxFlags,
    fstatfs, open, openat, openat2, readlinkat, statx,
};
use std::ffi::OsStr;
use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) struct Entity {
    pub dev: u64,
    pub ino: u64,
}
impl Entity {
    pub(crate) fn of(m: &Metadata) -> io::Result<Self> {
        if m.ino() == 0 {
            return Err(invalid("filesystem did not supply an inode identity"));
        }
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
        })
    }
}

/// Kernel observation for this held mount now (not a persistent filesystem UUID).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) struct Mount {
    pub(crate) id: u64,
    pub(crate) at_root: Option<bool>,
}
impl Mount {
    pub(crate) fn on(fd: &File) -> io::Result<Option<Self>> {
        let s = statx(
            fd,
            c"",
            AtFlags::EMPTY_PATH | AtFlags::NO_AUTOMOUNT | AtFlags::STATX_DONT_SYNC,
            StatxFlags::MNT_ID,
        )?;
        if s.stx_mask & StatxFlags::MNT_ID.bits() == 0 || s.stx_mnt_id == 0 {
            return Ok(None);
        }
        Ok(Some(Self {
            id: s.stx_mnt_id,
            at_root: s
                .stx_attributes_mask
                .contains(StatxAttributes::MOUNT_ROOT)
                .then(|| s.stx_attributes.contains(StatxAttributes::MOUNT_ROOT)),
        }))
    }
}

pub(crate) struct Pinned {
    path: PathBuf,
    kind: Kind,
    fd: File,
    chain: Vec<Entity>,
    no_automount: bool,
    local: Option<Arc<DiscoveryView>>,
}

fn dir_flags() -> OFlags {
    OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

/// Refuse a passive `O_PATH` pin before using it for another path component.
/// Linux can return an untriggered autofs FD for the last component of a
/// bare `O_PATH` lookup; going through it, or using DIRECTORY in the lookup,
/// could trigger the mount. Ordinary previous callers keep their flags.
pub(crate) fn require_no_automount(fd: &File) -> io::Result<()> {
    const AUTOFS_SUPER_MAGIC: u64 = 0x0187; // Linux UAPI <linux/magic.h>
    let s = statx(
        fd,
        c"",
        AtFlags::EMPTY_PATH | AtFlags::NO_AUTOMOUNT | AtFlags::STATX_DONT_SYNC,
        StatxFlags::TYPE,
    )?;
    if fstatfs(fd)?.f_type.cast_unsigned() == AUTOFS_SUPER_MAGIC
        || s.stx_attributes.contains(StatxAttributes::AUTOMOUNT)
    {
        return Err(invalid(
            "this offline action requires an already mounted local path, without an automount ancestor",
        ));
    }
    Ok(())
}

// Used for passive discovery only. Never follow the last name just to test
// whether it is an automount. Match every proved proc directory to this mount.
pub(crate) fn passive_proc_root() -> io::Result<(File, u64)> {
    let p = File::from(openat2(
        CWD,
        c"/proc",
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS | ResolveFlags::CACHED,
    )?);
    require_no_automount(&p)?;
    if !p.metadata()?.is_dir() || fstatfs(&p)?.f_type != PROC_SUPER_MAGIC {
        return Err(invalid("need the root of a genuine procfs"));
    }
    let m = Mount::on(&p)?
        .filter(|m| m.at_root == Some(true))
        .ok_or_else(|| invalid("procfs mount root unavailable"))?;
    let who = File::from(openat2(
        &p,
        c"self",
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        DISCOVERY_WITHIN | ResolveFlags::NO_XDEV,
    )?);
    if !who.metadata()?.file_type().is_symlink()
        || readlinkat(&who, c"", Vec::new())?.as_bytes()
            != std::process::id().to_string().as_bytes()
    {
        return Err(invalid("procfs refers to another process namespace"));
    }
    Ok((p, m.id))
}
pub(crate) fn passive_proc_child(
    parent: &File,
    name: &OsStr,
    mount: u64,
    directory: bool,
) -> io::Result<File> {
    let f = File::from(openat2(
        parent,
        name,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        DISCOVERY_WITHIN | ResolveFlags::NO_XDEV,
    )?);
    require_no_automount(&f)?;
    let m = f.metadata()?;
    if !(if directory { m.is_dir() } else { m.is_file() })
        || fstatfs(&f)?.f_type != PROC_SUPER_MAGIC
        || Mount::on(&f)?.is_none_or(|m| m.id != mount)
    {
        return Err(invalid(
            "procfs observation is not on the same genuine mount",
        ));
    }
    Ok(f)
}

/// Both mountinfo and fdinfo below belong to the actual calling task.
pub(crate) fn passive_task_proc() -> io::Result<(File, u64)> {
    let (mut root, mount) = passive_proc_root()?;
    let pid = std::process::id().to_string();
    let tid = rustix::thread::gettid().as_raw_pid().to_string();
    for name in [pid.as_str(), "task", tid.as_str()] {
        root = passive_proc_child(&root, OsStr::new(name), mount, true)?;
    }
    Ok((root, mount))
}

/// Obtain a mount ID from the kernel's own description table, not statx on
/// an unqualified mount (which could issue remote getattr/revalidation).
pub(crate) struct DiscoveryFdInfo {
    infos: File,
    mount: u64,
}
impl DiscoveryFdInfo {
    pub(crate) fn open() -> io::Result<Self> {
        let (task, mount) = passive_task_proc()?;
        Ok(Self {
            infos: passive_proc_child(&task, OsStr::new("fdinfo"), mount, true)?,
            mount,
        })
    }
    pub(crate) fn mount(&self, held: &File) -> io::Result<u64> {
        let name = held.as_raw_fd().to_string();
        let name = OsStr::new(&name);
        let prior = passive_proc_child(&self.infos, name, self.mount, false)?;
        let metadata = prior.metadata()?;
        let object = Entity::of(&metadata)?;
        let actual = File::from(openat2(
            &self.infos,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )?);
        if !actual.metadata()?.is_file()
            || Entity::of(&actual.metadata()?)? != object
            || fstatfs(&actual)?.f_type != PROC_SUPER_MAGIC
            || Mount::on(&actual)?.is_none_or(|m| m.id != self.mount)
        {
            return Err(invalid("untrusted descriptor mount observation"));
        }
        let mut bytes = Vec::new();
        actual.take(4097).read_to_end(&mut bytes)?;
        discovery_mount_id(&bytes)
    }
}
fn discovery_mount_id(bytes: &[u8]) -> io::Result<u64> {
    let no = || invalid("bounded original descriptor mount ID unavailable");
    if bytes.len() > 4096 || !bytes.ends_with(b"\n") {
        return Err(no());
    }
    let mut rows = bytes
        .split(|&c| c == b'\n')
        .filter_map(|r| r.strip_prefix(b"mnt_id:").map(<[u8]>::trim_ascii));
    let raw = rows.next().ok_or_else(no)?;
    let id = std::str::from_utf8(raw)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v != 0 && v.to_string().as_bytes() == raw)
        .ok_or_else(no)?;
    if rows.next().is_some() {
        return Err(no());
    }
    Ok(id)
}

const DISCOVERY_WITHIN: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS);
pub(crate) fn discover_component(parent: &File, name: &OsStr, crossing: bool) -> io::Result<File> {
    let path = OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    // From an already qualified local descriptor, ordinary same-mount
    // lookups may do local I/O. An expected named crossing needs a known
    // local mount-table row before this call and remains cache-only. Never
    // retry a rejected or unexpectedly uncached crossing.
    let policy = if crossing {
        ResolveFlags::CACHED
    } else {
        ResolveFlags::NO_XDEV
    };
    Ok(File::from(openat2(
        parent,
        name,
        path,
        Mode::empty(),
        DISCOVERY_WITHIN | policy,
    )?))
}

pub(crate) trait DiscoveryAdmission {
    fn root_mount(&self) -> io::Result<u64>;
    fn crossing(&self, parent: u64, prefix: &Path) -> io::Result<Option<u64>>;
    fn admit(&self, fd: &File) -> io::Result<u64>;
}

impl Pinned {
    pub(crate) fn open(path: &Path, kind: Kind) -> io::Result<Self> {
        Self::open_policy(path, kind, false)
    }
    pub(crate) fn already_mounted(path: &Path, kind: Kind) -> io::Result<Self> {
        Self::open_policy(path, kind, true)
    }
    fn open_policy(path: &Path, kind: Kind, no_automount: bool) -> io::Result<Self> {
        Self::open_guarded(path, kind, no_automount, None)
    }
    /// A guarded storage selection and all of its later rewalks use the
    /// same admitted current mount cut. Never turn this into an ordinary pin.
    pub(crate) fn local_storage(path: &Path, scope: &Arc<DiscoveryView>) -> io::Result<Self> {
        let mut one = Self::open_guarded(path, Kind::StorageRoot, true, Some(scope.as_ref()))?;
        one.local = Some(Arc::clone(scope));
        Ok(one)
    }
    #[cfg(test)]
    pub(crate) fn require_local_root(scope: &DiscoveryView) -> io::Result<()> {
        Self::open_guarded(Path::new("/"), Kind::Directory, true, Some(scope)).map(drop)
    }
    fn open_guarded(
        path: &Path,
        kind: Kind,
        no_automount: bool,
        admit: Option<&dyn DiscoveryAdmission>,
    ) -> io::Result<Self> {
        Self::walk(path, kind, no_automount, admit, |_, _, _| Ok(()))
    }
    fn walk(
        path: &Path,
        kind: Kind,
        no_automount: bool,
        admit: Option<&dyn DiscoveryAdmission>,
        mut see: impl FnMut(&File, Entity, usize) -> io::Result<()>,
    ) -> io::Result<Self> {
        let raw = path.as_os_str().as_bytes();
        kind.check_path(raw).map_err(invalid)?;
        let mut names = raw[1..].split(|b| *b == b'/').peekable();
        let (mut fd, mut known) = if let Some(check) = admit {
            let expected = check.root_mount()?;
            let root = File::from(openat2(
                CWD,
                c"/",
                OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS | ResolveFlags::CACHED,
            )?);
            if check.admit(&root)? != expected {
                return Err(invalid("original root mount changed during discovery"));
            }
            (root, expected)
        } else {
            (File::from(open("/", dir_flags(), Mode::empty())?), 0)
        };
        if no_automount {
            require_no_automount(&fd)?;
        }
        let mut chain = vec![Entity::of(&fd.metadata()?)?];
        see(&fd, chain[0], 0)?;
        let mut end = 0usize;
        if raw == b"/" {
            return Ok(Self {
                path: path.to_owned(),
                kind,
                fd,
                chain,
                no_automount,
                local: None,
            });
        }
        while let Some(name) = names.next() {
            let last_file = names.peek().is_none() && matches!(kind, Kind::Regular);
            let flags = if last_file || no_automount {
                OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC
            } else {
                dir_flags()
            };
            // Discovery first gates the live mount ID from this bare O_PATH
            // handle. Do not fstatfs, query directory metadata or walk below
            // it until the same local mount table admits that crossing.
            let child = if let Some(check) = admit {
                end += 1 + name.len();
                let prefix = Path::new(OsStr::from_bytes(&raw[..end]));
                let transition = check.crossing(known, prefix)?;
                let child = discover_component(&fd, OsStr::from_bytes(name), transition.is_some())?;
                let found = check.admit(&child)?;
                if found != transition.unwrap_or(known) {
                    return Err(invalid("unexpected mount crossing during discovery"));
                }
                known = found;
                child
            } else {
                File::from(openat(&fd, OsStr::from_bytes(name), flags, Mode::empty())?)
            };
            if no_automount {
                require_no_automount(&child)?;
            }
            let meta = child.metadata()?;
            if (last_file && !meta.is_file()) || (!last_file && !meta.is_dir()) {
                return Err(invalid(
                    "path must have the requested type; symbolic links and special files are refused",
                ));
            }
            let entity = Entity::of(&meta)?;
            if chain.contains(&entity) {
                return Err(invalid("aliased ancestor in selected path"));
            }
            see(&child, entity, chain.len())?;
            chain.push(entity);
            fd = child;
        }
        Ok(Self {
            path: path.to_owned(),
            kind,
            fd,
            chain,
            no_automount,
            local: None,
        })
    }

    pub(crate) fn entity(&self) -> Entity {
        *self.chain.last().expect("root and leaf are present")
    }
    #[cfg(test)]
    pub(crate) fn visible_chain(&self) -> &[Entity] {
        &self.chain
    }
    pub(crate) fn entity_is_visible_ancestor_of(&self, other: &Self) -> bool {
        other.chain[..other.chain.len() - 1].contains(&self.entity())
    }
    pub(crate) fn metadata(&self) -> io::Result<Metadata> {
        self.admit_held(&self.fd)?;
        self.fd.metadata()
    }

    #[cfg(test)]
    pub(crate) fn local_scope(&self) -> Option<&Arc<DiscoveryView>> {
        self.local.as_ref()
    }
    fn admission(&self) -> Option<&dyn DiscoveryAdmission> {
        self.local.as_deref().map(|v| v as &dyn DiscoveryAdmission)
    }
    pub(crate) fn admit_held(&self, fd: &File) -> io::Result<()> {
        if let Some(admit) = self.admission() {
            admit.admit(fd)?;
        }
        Ok(())
    }
    fn named(&self) -> io::Result<Self> {
        Self::open_guarded(&self.path, self.kind, self.no_automount, self.admission())
    }
    /// Visit only positively admitted same original prefixes, beginning at
    /// the visible root. No backward ".." traversal or retained FD per hop.
    pub(crate) fn same_local_chain(
        &self,
        mut visit: impl FnMut(&File, usize) -> io::Result<()>,
    ) -> io::Result<Mount> {
        let admit = self
            .admission()
            .ok_or_else(|| invalid("local ancestry requires a guarded selection"))?;
        self.same_admitted_chain(admit, |f, i| visit(f, i))
    }
    /// A fresh kernel admission can also revalidate a pre-existing original
    /// `O_PATH` for additional read-only policy, without replacing its identity
    /// or borrowing its lock. It must match every originally selected hop.
    pub(crate) fn same_admitted_chain(
        &self,
        admit: &dyn DiscoveryAdmission,
        mut visit: impl FnMut(&File, usize) -> io::Result<()>,
    ) -> io::Result<Mount> {
        let original = admit.admit(&self.fd)?;
        let named = Self::walk(&self.path, self.kind, true, Some(admit), |f, entity, i| {
            if self.chain.get(i) != Some(&entity) {
                return Err(invalid("original local ancestry changed"));
            }
            visit(f, i)
        })?;
        let mount = self.mount()?;
        if mount.id != original {
            return Err(invalid("original admission mount changed"));
        }
        check_named_mount(&self.chain, mount, &named.chain, named.mount()?)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    #[cfg(test)]
    pub(crate) fn passive_mount_observation(&self) -> bool {
        self.no_automount
    }
    pub(crate) fn mount(&self) -> io::Result<Mount> {
        self.admit_held(&self.fd)?;
        Mount::on(&self.fd)?.ok_or_else(|| invalid("live mount identity unavailable"))
    }

    /// Only a directory pin, not a general access lift. Its mount and entity
    /// must still match when opened relative to the original descriptor.
    pub(crate) fn relative_directory(&self) -> io::Result<File> {
        if !matches!(self.kind, Kind::StorageRoot) {
            return Err(invalid("expected an original directory pin"));
        }
        self.require_same_path()?;
        let out = File::from(openat(
            &self.fd,
            c".",
            OFlags::RDONLY
                | OFlags::DIRECTORY
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK
                | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let now = self.named()?;
        if self.chain != now.chain
            || self.mount()? != now.mount()?
            || Some(now.mount()?) != Mount::on(&out)?
            || now.entity() != Entity::of(&out.metadata()?)?
        {
            return Err(invalid("directory name or mount changed"));
        }
        Ok(out)
    }

    /// Return an independent, real open description for this selected storage
    /// directory. Never lock a duplicate of somebody else's open description.
    pub(crate) fn storage_directory(&self) -> io::Result<File> {
        if !matches!(self.kind, Kind::StorageRoot) {
            return Err(invalid("expected a selected storage root"));
        }
        let now = self.named()?;
        let original_mount = self.mount()?;
        if self.chain != now.chain || now.mount()?.id != original_mount.id {
            return Err(invalid(
                "original storage path, visible ancestry or mount changed",
            ));
        }
        // The fresh name is evidence only. Open "." relative to the FIRST held
        // directory, so revalidation never changes the object receiving the open.
        let flags = OFlags::RDONLY
            | OFlags::DIRECTORY
            | OFlags::NOFOLLOW
            | OFlags::NOATIME
            | OFlags::CLOEXEC;
        let f = File::from(if self.local.is_some() {
            openat2(
                &self.fd,
                c".",
                flags,
                Mode::empty(),
                DISCOVERY_WITHIN | ResolveFlags::NO_XDEV,
            )?
        } else {
            openat(&self.fd, c".", flags, Mode::empty())?
        });
        self.admit_held(&f)?;
        if self.entity() != Entity::of(&f.metadata()?)?
            || Mount::on(&f)?.is_none_or(|m| m.id != original_mount.id)
        {
            return Err(invalid(
                "opened description does not designate the selected storage directory",
            ));
        }
        Ok(f)
    }

    pub(crate) fn require_same_path(&self) -> io::Result<()> {
        let now = self.named()?;
        if now.chain != self.chain {
            return Err(invalid(
                "selected path or parent changed during observation",
            ));
        }
        Ok(())
    }

    /// Rewalk using `O_PATH` and require the original leaf mount as well as all
    /// visible entities. A bind of the same inode is not the original mount.
    /// The newly named FD is evidence only; keep using the first held FD.
    pub(crate) fn require_same_named_mount(&self) -> io::Result<Mount> {
        let now = self.named()?;
        check_named_mount(&self.chain, self.mount()?, &now.chain, now.mount()?)
    }
}

/// The same mount admission is mandatory for every walk and held read. No
/// methods expose the inner Pinned to a path opener lacking the live gate.
#[cfg(test)]
pub(crate) struct DiscoveryPin<'a> {
    original: Pinned,
    admit: &'a dyn DiscoveryAdmission,
}
#[cfg(test)]
impl<'a> DiscoveryPin<'a> {
    pub(crate) fn select(path: &Path, admit: &'a dyn DiscoveryAdmission) -> io::Result<Self> {
        Ok(Self {
            original: Pinned::open_guarded(path, Kind::StorageRoot, true, Some(admit))?,
            admit,
        })
    }
    pub(crate) fn require_named(&self) -> io::Result<Mount> {
        let x = &self.original;
        let named = Pinned::open_guarded(&x.path, x.kind, true, Some(self.admit))?;
        self.admit.admit(&x.fd)?;
        check_named_mount(&x.chain, x.mount()?, &named.chain, named.mount()?)
    }
    pub(crate) fn metadata(&self) -> io::Result<Metadata> {
        self.admit.admit(&self.original.fd)?;
        self.original.metadata()
    }
    pub(crate) fn magic(&self) -> io::Result<u32> {
        self.admit.admit(&self.original.fd)?;
        local_data_filesystem(&self.original.fd)
    }
    pub(crate) fn directory(&self) -> io::Result<File> {
        let mount = self.require_named()?;
        let f = File::from(openat(
            &self.original.fd,
            c".",
            OFlags::RDONLY
                | OFlags::DIRECTORY
                | OFlags::NOFOLLOW
                | OFlags::NOATIME
                | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        self.require_directory(&f, mount)?;
        Ok(f)
    }
    pub(crate) fn require_directory(&self, f: &File, mount: Mount) -> io::Result<()> {
        self.admit.admit(f)?;
        if Entity::of(&f.metadata()?)? != self.original.entity()
            || Mount::on(f)? != Some(mount)
            || self.require_named()? != mount
        {
            return Err(invalid("the visible discovery directory or mount changed"));
        }
        Ok(())
    }
}

pub(crate) fn same_mount_leaf(parent: &File, name: &OsStr) -> io::Result<File> {
    Ok(File::from(openat2(
        parent,
        name,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        DISCOVERY_WITHIN | ResolveFlags::NO_XDEV,
    )?))
}

fn check_named_mount(
    original: &[Entity],
    mount: Mount,
    named: &[Entity],
    current: Mount,
) -> io::Result<Mount> {
    if original != named || current != mount {
        return Err(invalid("selected name, ancestry or original mount changed"));
    }
    Ok(mount)
}

/// Keeps the original selected comparison operand and its admitted filesystem.
pub(crate) struct LocalDataFile {
    pin: Pinned,
    magic: u32,
}
impl LocalDataFile {
    pub(crate) fn select(path: &Path) -> io::Result<Self> {
        let pin = Pinned::open(path, Kind::Regular)?;
        let magic = local_data_filesystem(&pin.fd)?;
        Ok(Self { pin, magic })
    }

    pub(crate) fn metadata(&self) -> io::Result<Metadata> {
        self.pin.metadata()
    }
    pub(crate) fn require_same_path(&self) -> io::Result<()> {
        self.pin.require_same_path()
    }
    pub(crate) fn require_named_mount(&self) -> io::Result<Mount> {
        self.pin.require_same_named_mount()
    }

    // Selection keeps the admitted regular-file handle alive. Reopening the operand's
    // pathname instead could read a different object or open a device or FIFO.
    pub(crate) fn reader(&self) -> io::Result<File> {
        reopen_admitted_reader(
            &self.pin.fd,
            self.pin.entity(),
            ReaderBoundary::DataFilesystem(self.magic),
        )
    }
}

/// A fixed 101-byte SQLite claim, only below a separately authorized and
/// held storage directory. Never a data/backup/health or caller-path reader.
#[cfg(test)]
pub(crate) fn sqlite_claim_reader(pin: &File, entity: Entity, mount: u64) -> io::Result<File> {
    selected_claim_reader(pin, entity, mount, false)
}

#[cfg(test)]
pub(crate) fn discovery_claim_reader(pin: &File, entity: Entity, mount: u64) -> io::Result<File> {
    selected_claim_reader(pin, entity, mount, true)
}

#[cfg(test)]
fn selected_claim_reader(
    pin: &File,
    entity: Entity,
    mount: u64,
    passive: bool,
) -> io::Result<File> {
    let m = pin.metadata()?;
    if !rustix::fs::fcntl_getfl(pin)?.contains(OFlags::PATH)
        || !m.is_file()
        || m.mode() & 0o7777 != 0o600
        || m.nlink() != 1
        || m.len() != 101
        || Entity::of(&m)? != entity
        || Mount::on(pin)?.is_none_or(|v| v.id != mount)
    {
        return Err(invalid("not an original SQLite enrollment marker"));
    }
    reopen_admitted_reader(
        pin,
        entity,
        if passive {
            ReaderBoundary::Mount(mount, MountRead::PassiveSource)
        } else {
            ReaderBoundary::Mount(mount, MountRead::SelectedControl)
        },
    )
}

/// Reopen the exact, already-admitted external import operand, never its name.
pub(crate) fn external_import_reader(pin: &File, original: Entity, mount: u64) -> io::Result<File> {
    let meta = pin.metadata()?;
    if !rustix::fs::fcntl_getfl(pin)?.contains(OFlags::PATH)
        || !meta.is_file()
        || meta.nlink() == 0
        || Entity::of(&meta)? != original
        || Mount::on(pin)?.is_none_or(|now| now.id != mount)
    {
        return Err(invalid(
            "external import no longer selects the original local file",
        ));
    }
    reopen_admitted_reader(
        pin,
        original,
        ReaderBoundary::Mount(mount, MountRead::PassiveSource),
    )
}

/// Only the root-owned nonsecret live configuration or bound record below a separately
/// admitted instance root. The worker may update atime; no arbitrary path.
#[cfg(test)]
pub(crate) fn public_record_reader(pin: &File, mount: u64, length: u64) -> io::Result<File> {
    let m = pin.metadata()?;
    if !rustix::fs::fcntl_getfl(pin)?.contains(OFlags::PATH)
        || !m.is_file()
        || m.uid() != 0
        || m.mode() & 0o7777 != 0o444
        || m.nlink() != 1
        || length == 0
        || length > 1 << 20
        || m.len() != length
        || Mount::on(pin)?.is_none_or(|v| v.id != mount)
    {
        return Err(invalid("not the selected root-owned public origin record"));
    }
    reopen_admitted_reader(
        pin,
        Entity::of(&m)?,
        ReaderBoundary::Mount(mount, MountRead::PublicRecord),
    )
}

/// Upgrade an original `O_PATH` private regular inode only; never use its mutable pathname.
/// The caller must independently recheck the original name, held birth, protocol and lock.
pub(crate) fn private_inode_reader(pin: &File, uid: u32, expected_mount: u64) -> io::Result<File> {
    let m = pin.metadata()?;
    if !rustix::fs::fcntl_getfl(pin)?.contains(OFlags::PATH)
        || !m.is_file()
        || m.uid() != uid
        || m.nlink() != 1
        || m.mode() & 0o7777 != 0o600
        || Mount::on(pin)?.is_none_or(|v| v.id != expected_mount)
    {
        return Err(invalid(
            "expected a singly linked private file pinned on its original mount",
        ));
    }
    let file = reopen_admitted_reader(
        pin,
        Entity::of(&m)?,
        ReaderBoundary::DataFilesystem(local_data_filesystem(pin)?),
    )?;
    if Mount::on(&file)?.is_none_or(|v| v.id != expected_mount) {
        return Err(invalid("opened file no longer has the selected mount"));
    }
    Ok(file)
}

/// Public installed executables, libraries or root policy only. Caller walked the
/// entire fixed, protected root-owned chain and must recheck it after use.
pub(crate) fn installed_regular_reader(pin: &File) -> io::Result<File> {
    let m = pin.metadata()?;
    if !rustix::fs::fcntl_getfl(pin)?.contains(OFlags::PATH)
        || !m.is_file()
        || m.uid() != 0
        || m.mode() & 0o6022 != 0
        || m.nlink() == 0
    {
        return Err(invalid(
            "not an eligible root-controlled installation input",
        ));
    }
    reopen_admitted_reader(
        pin,
        Entity::of(&m)?,
        ReaderBoundary::InstalledRegular(local_data_filesystem(pin)?),
    )
}

// Unknown comparison inputs still have their prior filesystem allowlist. The other
// caller only reads fixed private control names below a separately selected, held
// storage directory. A current mount ID binds its descriptors, not its provenance.
#[derive(Copy, Clone)]
enum MountRead {
    PassiveSource,
    #[cfg(test)]
    SelectedControl,
    #[cfg(test)]
    PublicRecord,
}
#[derive(Copy, Clone)]
enum ReaderBoundary {
    DataFilesystem(u32),
    Mount(u64, MountRead),
    InstalledRegular(u32),
}
impl ReaderBoundary {
    fn matches(self, fd: &File) -> io::Result<bool> {
        match self {
            Self::DataFilesystem(magic) | Self::InstalledRegular(magic) => {
                Ok(filesystem(fd)? == Some(magic))
            }
            Self::Mount(id, _) => Ok(Mount::on(fd)?.is_some_and(|m| m.id == id)),
        }
    }
    fn uses_passive_proc(self) -> bool {
        matches!(self, Self::Mount(_, MountRead::PassiveSource))
    }
    fn permits_atime(self) -> bool {
        #[cfg(test)]
        if matches!(self, Self::Mount(_, MountRead::PublicRecord)) {
            return true;
        }
        matches!(self, Self::InstalledRegular(_))
    }
}

fn original_fd_directory(passive: bool) -> io::Result<(File, Option<u64>)> {
    let pid = std::process::id().to_string();
    let resolved = if passive {
        let (proc, mount) = passive_proc_root()?;
        let proc_pid = passive_proc_child(&proc, OsStr::new(&pid), mount, true)?;
        (
            passive_proc_child(&proc_pid, OsStr::new("fd"), mount, true)?,
            Some(mount),
        )
    } else {
        let proc = open("/proc", dir_flags(), Mode::empty())?;
        if fstatfs(&proc)?.f_type != PROC_SUPER_MAGIC {
            return Err(invalid("a genuine procfs for this process is required"));
        }
        if readlinkat(&proc, "self", Vec::new())?.as_bytes() != pid.as_bytes() {
            return Err(invalid("procfs refers to another process namespace"));
        }
        let proc_pid = openat(&proc, pid.as_str(), dir_flags(), Mode::empty())?;
        let fds = File::from(openat(&proc_pid, "fd", dir_flags(), Mode::empty())?);
        if fstatfs(&fds)?.f_type != PROC_SUPER_MAGIC {
            return Err(invalid("untrusted descriptor directory"));
        }
        (fds, None)
    };
    Ok(resolved)
}

fn reopen_admitted_reader(
    pin: &File,
    entity: Entity,
    boundary: ReaderBoundary,
) -> io::Result<File> {
    let (fds, passive_mount) = original_fd_directory(boundary.uses_passive_proc())?;
    // Even a procfs directory must belong to our descriptor table. Follow the kernel
    // link for the directory's own held fd and compare identities before using it.
    let myself = fds.as_raw_fd().to_string();
    let flags = OFlags::PATH
        | OFlags::CLOEXEC
        | if passive_mount.is_some() {
            OFlags::empty()
        } else {
            OFlags::DIRECTORY
        };
    let again = File::from(openat(&fds, myself.as_str(), flags, Mode::empty())?);
    if passive_mount.is_some() {
        require_no_automount(&again)?;
    }
    if Entity::of(&again.metadata()?)? != Entity::of(&fds.metadata()?)?
        || passive_mount
            .is_some_and(|id| Mount::on(&again).ok().flatten().is_none_or(|m| m.id != id))
    {
        return Err(invalid("procfs descriptor directory is not this process's"));
    }
    let number = pin.as_raw_fd().to_string();
    let flags = OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let proc_link = File::from(if passive_mount.is_some() {
        openat2(
            &fds,
            number.as_str(),
            flags,
            Mode::empty(),
            DISCOVERY_WITHIN | ResolveFlags::NO_XDEV,
        )?
    } else {
        openat(&fds, number.as_str(), flags, Mode::empty())?
    });
    if passive_mount.is_some() {
        require_no_automount(&proc_link)?;
    }
    if !proc_link.metadata()?.file_type().is_symlink()
        || fstatfs(&proc_link)?.f_type != PROC_SUPER_MAGIC
        || passive_mount.is_some_and(|id| {
            Mount::on(&proc_link)
                .ok()
                .flatten()
                .is_none_or(|m| m.id != id)
        })
    {
        return Err(invalid("descriptor is not a procfs kernel link"));
    }
    let check = File::from(openat(
        &fds,
        number.as_str(),
        OFlags::PATH | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let original = check.metadata()?;
    if !original.is_file() || Entity::of(&original)? != entity || !boundary.matches(&check)? {
        return Err(invalid(
            "descriptor no longer selects the original regular file",
        ));
    }
    let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC;
    let flags = if boundary.permits_atime() {
        flags
    } else {
        flags | OFlags::NOATIME
    };
    let result = File::from(openat(&fds, number.as_str(), flags, Mode::empty())?);
    let observed = result.metadata()?;
    if !observed.is_file() || Entity::of(&observed)? != entity {
        return Err(invalid(
            "read descriptor did not resolve to the original regular file",
        ));
    }
    if !boundary.matches(&result)? {
        return Err(invalid(
            "read descriptor no longer belongs to the selected data filesystem",
        ));
    }
    Ok(result)
}

pub(crate) fn local_data_filesystem(fd: &File) -> io::Result<u32> {
    require_local_data_fs(filesystem(fd)?)
}

// S_IFREG on its own does not make a kernel/remote control endpoint a data file.
// These are Linux UAPI values, not distro/mount-point names. Unknown means no reader.
fn require_local_data_fs(magic: Option<u32>) -> io::Result<u32> {
    match magic {
        Some(kind) if helper_linux_evidence::supported_local_magic(kind) => Ok(kind),
        _ => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "requires a supported local data filesystem (ext2/3/4, XFS, Btrfs or F2FS)",
        )),
    }
}

fn filesystem(file: &File) -> io::Result<Option<u32>> {
    Ok(u32::try_from(fstatfs(file)?.f_type).ok())
}

#[cfg(test)]
mod tests {
    use super::require_local_data_fs;
    use linux_raw_sys::general as fs;
    #[test]
    fn stateless_discovery_requires_one_canonical_kernel_fdinfo_mount_id() {
        use super::discovery_mount_id;
        assert_eq!(
            discovery_mount_id(b"pos:\t0\nflags:\t012000000\nmnt_id:\t18\nino:\t100\n").unwrap(),
            18
        );
        for raw in [
            &b"ino:\t18\n"[..],
            b"mnt_id:\t0\n",
            b"mnt_id:\t018\n",
            b"mnt_id:\t18",
            b"mnt_id:\t18\nmnt_id:\t18\n",
            b"mnt_id:\t18 19\n",
        ] {
            assert!(discovery_mount_id(raw).is_err());
        }
    }
    #[test]
    fn nonempty_v17_current_task_fdinfo_works_nondumpable_without_extra_authority() {
        use super::{DiscoveryFdInfo, Mount, passive_task_proc};
        use rustix::process::{DumpableBehavior, dumpable_behavior, getuid, set_dumpable_behavior};
        const CHILD: &str = "HELPER_UNIT_H03_FDINFO_CHILD";
        if std::env::var_os(CHILD).is_none() {
            // The flag affects only this dedicated fixture process, never the
            // outer test harness, any operator, or system configuration.
            let status=std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact","pinned::tests::nonempty_v17_current_task_fdinfo_works_nondumpable_without_extra_authority"])
                .env(CHILD,"1").status().unwrap();
            assert!(status.success());
            return;
        }
        assert!(
            !getuid().is_root(),
            "this fixture requires the ordinary unprivileged build runner"
        );
        set_dumpable_behavior(DumpableBehavior::NotDumpable).unwrap();
        assert_eq!(dumpable_behavior().unwrap(), DumpableBehavior::NotDumpable);
        let area = tempfile::tempdir().unwrap();
        let (proc, proc_id) = passive_task_proc().unwrap();
        let scratch = std::fs::File::open(area.path()).unwrap();
        let disk_id = Mount::on(&scratch).unwrap().unwrap().id;
        assert_ne!(proc_id, disk_id);
        let gate = DiscoveryFdInfo::open().unwrap();
        assert_eq!(gate.mount(&scratch).unwrap(), disk_id);
        assert_eq!(gate.mount(&proc).unwrap(), proc_id);
    }
    #[test]
    fn nonempty_v17_guard_stops_before_inspecting_the_denied_component() {
        use super::{DiscoveryAdmission, DiscoveryFdInfo, Kind, Pinned};
        use rustix::fs::{Mode, OFlags, open};
        use std::{cell::Cell, fs::File, io, path::Path};
        struct Deny {
            probe: DiscoveryFdInfo,
            start: u64,
            asked: Cell<bool>,
            admitted: Cell<u8>,
        }
        impl DiscoveryAdmission for Deny {
            fn root_mount(&self) -> io::Result<u64> {
                Ok(self.start)
            }
            fn crossing(&self, parent: u64, prefix: &Path) -> io::Result<Option<u64>> {
                assert_eq!(parent, self.start);
                assert_eq!(prefix, Path::new("/h03-fabricated-remote"));
                self.asked.set(true);
                Err(io::ErrorKind::Unsupported.into())
            }
            fn admit(&self, who: &File) -> io::Result<u64> {
                self.admitted.set(self.admitted.get() + 1);
                self.probe.mount(who)
            }
        }
        let probe = DiscoveryFdInfo::open().unwrap();
        let root = File::from(open(c"/", OFlags::PATH | OFlags::CLOEXEC, Mode::empty()).unwrap());
        let admission = Deny {
            start: probe.mount(&root).unwrap(),
            probe,
            asked: Cell::new(false),
            admitted: Cell::new(0),
        };
        let visited = Cell::new(0);
        let observed = Pinned::walk(
            Path::new("/h03-fabricated-remote/any"),
            Kind::StorageRoot,
            true,
            Some(&admission),
            |_, _, _| {
                visited.set(visited.get() + 1);
                Ok(())
            },
        );
        assert_eq!(observed.err().unwrap().kind(), io::ErrorKind::Unsupported);
        assert!(admission.asked.get());
        assert_eq!(admission.admitted.get(), 1);
        assert_eq!(visited.get(), 1);
    }
    #[test]
    fn same_inodes_do_not_substitute_another_named_mount() {
        use super::{Entity, Mount, check_named_mount};
        let originals = [Entity { dev: 1, ino: 2 }, Entity { dev: 3, ino: 4 }];
        let mount = Mount {
            id: 10,
            at_root: Some(true),
        };
        assert_eq!(
            check_named_mount(&originals, mount, &originals, mount).unwrap(),
            mount
        );
        assert!(
            check_named_mount(&originals, mount, &originals, Mount { id: 11, ..mount }).is_err()
        );
        let other_parent = [Entity { dev: 1, ino: 5 }, originals[1]];
        assert!(check_named_mount(&originals, mount, &other_parent, mount).is_err());
    }
    #[test]
    fn new_directory_at_the_same_spelling_cannot_replace_a_held_observation() {
        let area = tempfile::tempdir().unwrap();
        let dir = area.path().join("original");
        std::fs::create_dir(&dir).unwrap();
        let original = super::Pinned::open(&dir, super::Kind::Directory).unwrap();
        assert_eq!(
            original.require_same_named_mount().unwrap(),
            original.mount().unwrap()
        );
        std::fs::rename(&dir, area.path().join("retained")).unwrap();
        std::fs::create_dir(&dir).unwrap();
        assert!(original.require_same_path().is_err());
        assert!(original.require_same_named_mount().is_err());
        assert_ne!(
            original.entity(),
            super::Pinned::open(&dir, super::Kind::Directory)
                .unwrap()
                .entity()
        );
    }
    #[test]
    fn replacing_only_a_parent_is_rejected_even_when_the_original_leaf_is_moved_back() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("original-parent");
        let saved_parent = temp.path().join("saved-parent");
        let leaf = parent.join("unchanged-leaf");
        std::fs::create_dir(&parent).unwrap();
        std::fs::create_dir(&leaf).unwrap();
        let selected = super::Pinned::open(&leaf, super::Kind::Directory).unwrap();
        let original_parent = super::Pinned::open(&parent, super::Kind::Directory).unwrap();
        std::fs::rename(&parent, &saved_parent).unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::rename(saved_parent.join("unchanged-leaf"), &leaf).unwrap();

        let current = super::Pinned::open(&leaf, super::Kind::Directory).unwrap();
        assert_eq!(selected.entity(), current.entity()); // comparing just the leaf would pass
        assert_ne!(
            original_parent.entity(),
            super::Pinned::open(&parent, super::Kind::Directory)
                .unwrap()
                .entity()
        );
        assert!(selected.require_same_path().is_err());
        assert!(current.require_same_path().is_ok());
    }
    #[test]
    fn reopening_a_pinned_public_object_does_not_open_its_replacement_by_name() {
        use super::{
            Entity, Kind, Mount, MountRead, Pinned, ReaderBoundary, reopen_admitted_reader,
        };
        use std::io::Read;
        let area = tempfile::tempdir().unwrap();
        let named = area.path().join("original");
        let retained = area.path().join("retained");
        std::fs::write(&named, b"selected bytes").unwrap();
        let old = Pinned::open(&named, Kind::Regular).unwrap();
        let id = Mount::on(&old.fd).unwrap().unwrap().id;
        std::fs::rename(&named, retained).unwrap();
        std::os::unix::fs::symlink("does-not-exist", &named).unwrap();
        let mut reopened = reopen_admitted_reader(
            &old.fd,
            old.entity(),
            ReaderBoundary::Mount(id, MountRead::PublicRecord),
        )
        .unwrap();
        let mut b = Vec::new();
        reopened.read_to_end(&mut b).unwrap();
        assert_eq!(b, b"selected bytes");
        assert_eq!(
            Entity::of(&reopened.metadata().unwrap()).unwrap(),
            old.entity()
        );
        assert!(old.require_same_path().is_err());
    }
    #[test]
    fn filesystem_allowlist_returns_the_admitted_type_or_rejects_it() {
        for v in [
            fs::EXT2_SUPER_MAGIC,
            fs::EXT3_SUPER_MAGIC,
            fs::EXT4_SUPER_MAGIC,
            fs::XFS_SUPER_MAGIC,
            fs::BTRFS_SUPER_MAGIC,
            fs::F2FS_SUPER_MAGIC,
        ] {
            assert_eq!(require_local_data_fs(Some(v)).unwrap(), v);
        }
        for rejected in [
            None,
            Some(0),
            Some(u32::MAX),
            Some(fs::PROC_SUPER_MAGIC),
            Some(fs::SYSFS_MAGIC),
            Some(fs::DEBUGFS_MAGIC),
            Some(fs::TRACEFS_MAGIC),
            Some(fs::TMPFS_MAGIC),
            Some(fs::NFS_SUPER_MAGIC),
            Some(fs::CIFS_SUPER_MAGIC),
            Some(fs::FUSE_SUPER_MAGIC),
            Some(fs::OVERLAYFS_SUPER_MAGIC),
        ] {
            assert!(require_local_data_fs(rejected).is_err());
        }
    }
}
