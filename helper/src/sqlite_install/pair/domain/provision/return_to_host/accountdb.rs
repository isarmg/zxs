//! Host-side evidence for the future native SAM account-store path. Never
//! opens TDB through Samba, reads its content, creates a store or grants work.
//! An empty directory is only an observation, not proof of a first install.
use super::{Site, approval, bad, layout, lease, mark, pin_dir, safe_dir};
use crate::{pinned::{Entity, Pinned}, sqlite_install::request::Id};
use rustix::{fs::{Mode, OFlags, RawDir, openat}, io::Errno};
use std::{ffi::CStr, fs::{File, Metadata}, io, mem::MaybeUninit, os::unix::fs::MetadataExt};

const DIRECTORY: &CStr = c"accountdb";
const DATABASE: &CStr = c"passdb.tdb";
const LIMIT: u64 = 64 << 20; // resource limit, not a recognition of any TDB version

type Location = (Entity, u64);
type Subject = (Id, u32, u32);

#[derive(Clone, Copy, Eq, PartialEq)]
struct Stamp {
    place: Location,
    uid: u32,
    gid: u32,
    mode: u32,
    links: u64,
    length: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
impl Stamp {
    fn on(f: &File) -> io::Result<Self> {
        let m = f.metadata()?;
        Ok(Self::read(mark(f)?, &m))
    }
    fn read(place: Location, m: &Metadata) -> Self {
        Self { place, uid: m.uid(), gid: m.gid(), mode: m.mode(), links: m.nlink(), length: m.len(),
            mtime: (m.mtime(), m.mtime_nsec()), ctime: (m.ctime(), m.ctime_nsec()) }
    }
    fn child_of(self, parent: Location) -> bool {
        self.place.0 != parent.0 && self.place.0.dev == parent.0.dev && self.place.1 == parent.1
            && self.uid == 0 && self.gid == 0
    }
    fn directory(self, parent: Location) -> bool {
        self.child_of(parent) && self.mode == 0o040700 && self.links > 0
    }
    fn database(self, parent: Location) -> bool {
        self.child_of(parent) && self.mode == 0o100600 && self.links == 1 && (1..=LIMIT).contains(&self.length)
    }
}

fn raw_directory(pin: &File) -> io::Result<File> {
    Ok(File::from(openat(pin, c".", OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW
        | OFlags::NOATIME | OFlags::CLOEXEC | OFlags::NONBLOCK, Mode::empty())?))
}
fn name(dir: &File) -> io::Result<Option<File>> {
    match openat(dir, DATABASE, OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty()) {
        Ok(fd) => Ok(Some(File::from(fd))),
        Err(Errno::NOENT) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

// Names are fixed raw bytes, not a spelling synthesized by successful lookup
// on a case-folding filesystem. No contents and no other entries are opened.
fn inventory(parent: &File, start: Stamp) -> io::Result<bool> {
    let d = raw_directory(parent)?;
    let stable = || -> io::Result<()> {
        if Stamp::on(parent)? != start || Stamp::on(&d)? != start {
            return Err(bad("the original account directory changed during observation"));
        }
        Ok(())
    };
    stable()?;
    let mut present = false;
    let mut buf = [MaybeUninit::uninit(); 4096];
    let mut records = RawDir::new(&d, &mut buf);
    let mut remaining = 3;
    while let Some(row) = records.next() {
        if remaining == 0 { return Err(bad("extra records in the original account directory")); }
        remaining -= 1;
        let row = row?;
        match row.file_name().to_bytes() {
            b"." | b".." => (),
            b"passdb.tdb" if !present => present = true,
            _ => return Err(bad("unknown entry in the original account directory; no takeover")),
        }
    }
    stable()?;
    Ok(present)
}
fn named(dir: &File, expected: Stamp) -> io::Result<()> {
    let f = name(dir)?.ok_or_else(|| bad("the original passdb name is missing"))?;
    if Stamp::on(&f)? != expected { return Err(bad("the named passdb no longer matches the held original")); }
    Ok(())
}

fn originals(system: &Pinned, system_ex: &File, site: &Site, site_ex: &File, subject: Subject) -> io::Result<()> {
    layout::verify_registered_instance(site, subject.2)?;
    if approval::profile()? != subject { return Err(bad("host maintenance subject changed")); }
    // Independently reopen the already selected pin only for path/mount
    // corroboration. This is NOT a new lock, and no database is opened.
    let current_system = system.storage_directory()?;
    let s = current_system.metadata()?;
    if s.uid() != subject.1 || s.mode() != 0o040700 || s.nlink() == 0
        || system.visible_chain().contains(&mark(site.base())?.0)
        || site.chain.iter().map(mark).collect::<io::Result<Vec<_>>>()?.iter().any(|r| r.0 == system.entity()) {
        return Err(bad("wrong original SYSTEM or overlapping host installation"));
    }
    lease(system_ex, mark(&current_system)?)?;
    lease(site_ex, mark(site.base())?)?;
    Ok(())
}

pub(super) struct AccountParent<'a> {
    system: &'a Pinned,
    system_ex: &'a File,
    site: &'a Site,
    site_ex: &'a File,
    dir: File,
    location: Location,
    subject: Subject,
}
pub(super) enum Seen<'a, 'orig> { NotPresent, Present(Store<'a, 'orig>) }
pub(super) struct Store<'a, 'orig> {
    parent: &'a AccountParent<'orig>,
    pin: File,
    file: Stamp,
    directory: Stamp,
}
impl<'orig> AccountParent<'orig> {
    /// Both supplied descriptions must actually own the original kernel EX
    /// (not two unlocked FDs plus someone else's lock). Never take or convert
    /// a long lock here. The holder must already have quiesced both services.
    pub(super) fn select(system: &'orig Pinned, system_ex: &'orig File, site: &'orig Site,
        site_ex: &'orig File) -> io::Result<Self> {
        site.check()?;
        let subject = approval::profile()?;
        originals(system, system_ex, site, site_ex, subject)?;
        let pin = pin_dir(site_ex, DIRECTORY)?;
        let shape = Stamp::on(&pin)?;
        if !shape.directory(mark(site_ex)?) { return Err(bad("expected the original root-only account directory")); }
        layout::exact_component(site_ex, DIRECTORY, &pin)?;
        let dir = raw_directory(&pin)?;
        if Stamp::on(&dir)? != shape { return Err(bad("different account directory during admission")); }
        let d = Self { system, system_ex, site, site_ex, dir, location: shape.place, subject };
        d.require()?;
        Ok(d)
    }

    fn require(&self) -> io::Result<()> {
        originals(self.system, self.system_ex, self.site, self.site_ex, self.subject)?;
        let pin = pin_dir(self.site_ex, DIRECTORY)?;
        layout::exact_component(self.site_ex, DIRECTORY, &pin)?;
        let real = Stamp::on(&self.dir)?;
        if real.place != self.location || !real.directory(mark(self.site_ex)?) || Stamp::on(&pin)? != real {
            return Err(bad("protected account directory or its original name changed"));
        }
        safe_dir(&self.dir)?;
        crate::pinned::local_data_filesystem(&self.dir)?;
        layout::bare_acl(&self.dir, c"system.posix_acl_access")?;
        layout::bare_acl(&self.dir, c"system.posix_acl_default")?;
        Ok(())
    }

    pub(super) fn observe(&self) -> io::Result<Seen<'_, 'orig>> {
        self.require()?;
        let directory = Stamp::on(&self.dir)?;
        let listed = inventory(&self.dir, directory)?;
        let result = match name(&self.dir)? {
            None if !listed => {
                self.require()?;
                if inventory(&self.dir, directory)? || name(&self.dir)?.is_some()
                    || Stamp::on(&self.dir)? != directory {
                    return Err(bad("the account directory changed before the absence was corroborated"));
                }
                Seen::NotPresent
            }
            Some(pin) if listed => {
                let file = Stamp::on(&pin)?;
                let result = Store { parent: self, pin, file, directory };
                result.require()?;
                Seen::Present(result)
            }
            _ => return Err(bad("fixed name lookup disagrees with the original account directory listing")),
        };
        Ok(result)
    }
}
impl Store<'_, '_> {
    pub(super) fn require(&self) -> io::Result<()> {
        let d = &self.parent.dir;
        self.parent.require()?;
        if !self.file.database(self.directory.place) || Stamp::on(&self.pin)? != self.file
            || !inventory(d, self.directory)? {
            return Err(bad("wrong, changed or unbounded original passdb"));
        }
        named(d, self.file)?;
        // Only on the verified original regular file and only for the normal
        // POSIX ACL absence check. No data read or native database open. This
        // supervisor does not itself hold any TDB handle or POSIX byte lock.
        let descriptor = crate::pinned::private_inode_reader(&self.pin, 0, self.file.place.1)?;
        if Stamp::on(&descriptor)? != self.file { return Err(bad("account database changed before mode check")); }
        layout::bare_acl(&descriptor, c"system.posix_acl_access")?;
        self.parent.require()?;
        named(d, self.file)?;
        if Stamp::on(&self.pin)? != self.file || !inventory(d, self.directory)? {
            return Err(bad("account data or its original parent changed"));
        }
        named(d, self.file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn only_the_exact_named_store_and_still_held_inode_are_observations() {
        let area = tempfile::tempdir().unwrap();
        let d = area.path().join("accountdb");
        fs::create_dir(&d).unwrap();
        let f = File::open(&d).unwrap();
        assert!(!inventory(&f, Stamp::on(&f).unwrap()).unwrap());
        fs::write(d.join("PASSDB.TDB"), b"not a database").unwrap();
        assert!(inventory(&f, Stamp::on(&f).unwrap()).is_err());
        fs::rename(d.join("PASSDB.TDB"), d.join("passdb.tdb")).unwrap();
        assert!(inventory(&f, Stamp::on(&f).unwrap()).unwrap());
        let pin = name(&f).unwrap().unwrap();
        let old = Stamp::on(&pin).unwrap();
        named(&f, old).unwrap();
        fs::rename(d.join("passdb.tdb"), area.path().join("retained")).unwrap();
        fs::write(d.join("passdb.tdb"), b"not a database").unwrap();
        assert_ne!(mark(&pin).unwrap(), mark(&name(&f).unwrap().unwrap()).unwrap());
        assert!(named(&f, old).is_err());
        fs::write(d.join("passdb.tdb.tmp"), b"unknown").unwrap();
        assert!(inventory(&f, Stamp::on(&f).unwrap()).is_err());
    }

    #[test]
    fn account_directory_and_database_require_exact_owner_permissions_and_mount() {
        let base = (Entity { dev: 77, ino: 1 }, 9);
        let dir = Stamp { place: (Entity { dev: 77, ino: 2 }, 9), uid: 0, gid: 0,
            mode: 0o040700, links: 2, length: 0, mtime: (1, 2), ctime: (3, 4) };
        let file = Stamp { place: (Entity { dev: 77, ino: 3 }, 9), mode: 0o100600, links: 1, length: 4096, ..dir };
        assert!(dir.directory(base));
        assert!(file.database(dir.place));
        for wrong in [Stamp { uid: 1000, ..dir }, Stamp { gid: 1000, ..dir },
            Stamp { mode: 0o040750, ..dir }, Stamp { place: (dir.place.0, 8), ..dir }] {
            assert!(!wrong.directory(base));
        }
        for wrong in [Stamp { uid: 1000, ..file }, Stamp { gid: 1000, ..file },
            Stamp { mode: 0o100644, ..file }, Stamp { mode: 0o120600, ..file }, Stamp { links: 2, ..file },
            Stamp { length: 0, ..file }, Stamp { length: LIMIT + 1, ..file },
            Stamp { place: (file.place.0, 7), ..file }, Stamp { place: dir.place, ..file }] {
            assert!(!wrong.database(dir.place));
        }
    }
}
