//! The only OS file reservation for the fresh SQLite path. Once closed, the raw
//! DB is NEVER reopened for inspection. Only metadata and the SQLite VFS are used.
use super::{
    DATABASE, bad, engine,
    originals::{Group, HeldRoot, LockedSystem},
    request::Request,
};
use crate::pinned::{Entity, Mount, same_mount_leaf};
use rustix::fs::{AtFlags, Mode, OFlags, StatxFlags, makedev, openat, statx};
use std::{ffi::OsStr, fs::File, io, os::unix::fs::MetadataExt, path::PathBuf};

pub(super) struct Reserved<'g>(DbName<'g>);
pub(super) struct Existing<'g>(DbName<'g>, DbUse);
struct DbName<'g> {
    root: HeldRoot<'g>,
    path: PathBuf,
    entity: Entity,
    mount: u64,
}
#[derive(Clone, Copy)]
enum DbUse {
    InitialGroup,
    InitialReceipt,
    AccountObservation,
}
impl<'g> Reserved<'g> {
    pub(super) fn create(group: &'g Group, request: &Request) -> io::Result<Self> {
        let path = request.database(engine::path_limit()?)?;
        if group.uid() != request.uid() || !group.matches(request) {
            return Err(bad("wrong approved group for original INIT"));
        }
        group.recheck()?;
        for i in 0..group.dirs().len() {
            group.scan_names(i, &[])?;
        }
        group.recheck()?;
        let s = group.system();
        // Only this open can CREATE the DB. Never truncate/adopt an existing
        // regular file, and never repair a half-built database or an unknown name.
        let new = File::from(openat(
            s,
            c"SYSTEM.sqlite3",
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        let meta = new.metadata()?;
        let mount = Mount::on(&new)?
            .ok_or_else(|| bad("new database has no mount identity"))?
            .id;
        let me = Self(DbName {
            root: group.system_source(),
            path,
            entity: Entity::of(&meta)?,
            mount,
        });
        if !meta.is_file()
            || meta.uid() != request.uid()
            || meta.mode() & 0o7777 != 0o600
            || meta.nlink() != 1
            || meta.len() != 0
            || !Mount::on(s)?.is_some_and(|m| m.id == mount)
            || s.metadata()?.dev() != meta.dev()
        {
            return Err(bad(
                "exclusive creation did not yield a new private database on the original SYSTEM",
            ));
        }
        me.require_name(true)?;
        new.sync_all()?;
        s.sync_all()?;
        me.require_name(true)?;
        // A raw close on an inode used by SQLite elsewhere in this process
        // would cancel SQLite POSIX locks. No connection exists in this stage.
        drop(new);
        group.scan_names(0, &[DATABASE.as_bytes()])?;
        group.recheck()?;
        me.require_name(true)?;
        Ok(me)
    }
    pub(super) fn path(&self) -> &std::path::Path {
        &self.0.path
    }
    pub(super) fn same_existing(&self, other: &Existing<'_>) -> bool {
        std::ptr::eq(self.0.root.dir(), other.0.root.dir())
            && self.0.entity == other.0.entity
            && self.0.mount == other.0.mount
    }
    pub(super) fn pristine(&self) -> io::Result<()> {
        if self.0.root.controls(&[DATABASE.as_bytes()])? != [true] {
            return Err(bad("missing reserved database"));
        }
        self.require_name(true)
    }
    pub(super) fn require_name(&self, empty: bool) -> io::Result<()> {
        self.0.check(empty).map(|_| ())
    }
    pub(super) fn sync_parent(&self) -> io::Result<()> {
        self.0.root.dir().sync_all()
    }
}
impl<'g> Existing<'g> {
    pub(super) fn observe(group: &'g Group, r: &Request) -> io::Result<Self> {
        if group.uid() != r.uid() || !group.matches(r) {
            return Err(bad("selected group does not match original INIT"));
        }
        let path = r.database(engine::path_limit()?)?;
        Self::from_source(group.system_source(), path, DbUse::InitialGroup)
    }
    /// Must never authorize a writable VFS. Pending INIT can legitimately
    /// exist before any original marker, so this admits its optional S slot.
    pub(super) fn initial_receipt(locked: &'g LockedSystem, r: &Request) -> io::Result<Self> {
        locked.require()?;
        let source = locked.source();
        if source.uid() != r.uid() || source.path() != r.paths()[0].as_path() {
            return Err(bad("wrong original SYSTEM for the requested INIT receipt"));
        }
        Self::from_source(
            source,
            r.database(engine::path_limit()?)?,
            DbUse::InitialReceipt,
        )
    }
    pub(super) fn observed_system(locked: &'g LockedSystem) -> io::Result<Self> {
        locked.require()?;
        let source = locked.source();
        let path = super::request::database_path(source.path(), engine::path_limit()?)?;
        Self::from_source(source, path, DbUse::AccountObservation)
    }
    fn from_source(root: HeldRoot<'g>, path: PathBuf, use_: DbUse) -> io::Result<Self> {
        let names = root.controls(&[DATABASE.as_bytes(), super::MARKER])?;
        if !names[0] || matches!(use_, DbUse::AccountObservation) && !names[1] {
            return Err(bad(
                "missing expected database or existing SYSTEM enrollment marker",
            ));
        }
        let s = database_metadata(root, StatxFlags::BASIC_STATS | StatxFlags::MNT_ID)?;
        let x = Self(
            DbName {
                root,
                path,
                entity: Entity {
                    dev: makedev(s.stx_dev_major, s.stx_dev_minor),
                    ino: s.stx_ino,
                },
                mount: s.stx_mnt_id,
            },
            use_,
        );
        x.require()?;
        Ok(x)
    }
    // Non-readable kernel pin for the fresh-only paired issuer. No raw
    // readable DB FD ever opens in root or in the VFS writer via this pin.
    pub(super) fn fresh_observation_pin(&self) -> io::Result<File> {
        use rustix::fs::{ResolveFlags, openat2};
        self.sidecars_absent()?;
        let fd = File::from(openat2(
            self.0.root.dir(),
            c"SYSTEM.sqlite3",
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH
                | ResolveFlags::NO_XDEV
                | ResolveFlags::NO_SYMLINKS
                | ResolveFlags::NO_MAGICLINKS,
        )?);
        let m = fd.metadata()?;
        if m.mode() != 0o100_600
            || m.nlink() != 1
            || m.uid() != self.0.root.uid()
            || m.len() == 0
            || Entity::of(&m)? != self.0.entity
            || Mount::on(&fd)?.is_none_or(|x| x.id != self.0.mount)
        {
            return Err(bad("different fresh INIT database pin"));
        }
        self.sidecars_absent()?;
        Ok(fd)
    }
    pub(super) fn matches_fresh_origin(&self, expected: [u64; 3]) -> io::Result<()> {
        self.sidecars_absent()?;
        let got = [self.0.entity.dev, self.0.entity.ino, self.0.mount];
        if got != expected {
            return Err(bad("the original account-free INIT database changed"));
        }
        Ok(())
    }
    pub(super) fn allows_install_write(&self) -> bool {
        matches!(self.1, DbUse::InitialGroup)
    }
    pub(super) fn path(&self) -> &std::path::Path {
        &self.0.path
    }
    pub(super) fn require(&self) -> io::Result<()> {
        let me = &self.0;
        let s = me.root.dir();
        if me.entity.ino == 0
            || me.entity.dev != s.metadata()?.dev()
            || Mount::on(s)?.is_none_or(|x| x.id != me.mount)
            || me.check(false)? == 0
        {
            return Err(bad("expected a positive-sized original database on SYSTEM"));
        }
        Ok(())
    }
    pub(super) fn sidecars_absent(&self) -> io::Result<()> {
        // This narrow action cannot repair a hot journal and never silently
        // opens its unknown source RW in order to identify that source.
        let names = self
            .0
            .root
            .controls(&[DATABASE.as_bytes(), super::MARKER])?;
        if !names[0] || matches!(self.1, DbUse::AccountObservation) && !names[1] {
            return Err(bad(
                "original database or required SYSTEM marker not present",
            ));
        }
        self.require()
    }
}
impl DbName<'_> {
    fn check(&self, empty: bool) -> io::Result<u64> {
        // The SQLite Unix VFS resolves an absolute canonical pathname. Match
        // the *current* named chain and mount to our already-held original S,
        // not just "SYSTEM.sqlite3" inside a now-detached directory.
        self.root.require()?;
        let need = StatxFlags::TYPE
            | StatxFlags::MODE
            | StatxFlags::NLINK
            | StatxFlags::UID
            | StatxFlags::INO
            | StatxFlags::SIZE
            | StatxFlags::MNT_ID;
        let s = database_metadata(self.root, need)?;
        if s.stx_mask & need.bits() != need.bits()
            || s.stx_mode != 0o100_600
            || s.stx_nlink != 1
            || s.stx_uid != self.root.uid()
            || s.stx_ino != self.entity.ino
            || makedev(s.stx_dev_major, s.stx_dev_minor) != self.entity.dev
            || s.stx_mnt_id != self.mount
            || empty && s.stx_size != 0
        {
            return Err(bad("original database reservation changed"));
        }
        Ok(s.stx_size)
    }
}
// Guarded diagnostic roots reject any bind/automount/symlink of the
// fixed DB name before issuing statx. The O_PATH is never a SQLite data FD.
fn database_metadata(root: HeldRoot<'_>, need: StatxFlags) -> io::Result<rustix::fs::Statx> {
    let flags = AtFlags::SYMLINK_NOFOLLOW | AtFlags::NO_AUTOMOUNT;
    if root.local_only() {
        let pin = same_mount_leaf(root.dir(), OsStr::new(DATABASE))?;
        crate::pinned::require_no_automount(&pin)?;
        Ok(statx(&pin, c"", flags | AtFlags::EMPTY_PATH, need)?)
    } else {
        Ok(statx(root.dir(), c"SYSTEM.sqlite3", flags, need)?)
    }
}
