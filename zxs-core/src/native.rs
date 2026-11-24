//! Linux ordinary-object boundaries. Never read-open a user name before O_PATH proof.
use crate::{
    Result, hex, invalid,
    r3::{Body, Digest},
};
use rustix::fs::{self, AtFlags, CWD, Dir, Mode, OFlags, ResolveFlags};
use rustix::io::Errno;
use rustix::process::{getegid, geteuid, getgid, getuid};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs::{File, Metadata},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStringExt,
            fs::{FileExt, MetadataExt},
        },
    },
    path::{Component, Path, PathBuf},
};

const CHUNK: usize = 1 << 20;
const SAFE: OFlags = OFlags::CLOEXEC.union(OFlags::NOFOLLOW);
const WALK: ResolveFlags = ResolveFlags::NO_MAGICLINKS.union(ResolveFlags::NO_SYMLINKS);
const INSIDE: ResolveFlags = WALK
    .union(ResolveFlags::BENEATH)
    .union(ResolveFlags::NO_XDEV);

pub(crate) fn ordinary_uid() -> Result<u32> {
    let uid = getuid().as_raw();
    if uid == 0 || getuid() != geteuid() || getgid() != getegid() {
        return Err(invalid(
            "use an ordinary Linux account without set-id elevation",
        ));
    }
    let (proc, pid) = proc_root()?;
    let pointed = fs::readlinkat(&proc, "thread-self", Vec::new())?;
    let path = pointed.to_str()?;
    let base = format!("{pid}/task/");
    if !path
        .strip_prefix(&base)
        .is_some_and(|tid| !tid.is_empty() && tid.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(invalid("native procfs cannot attest this thread"));
    }
    let status = format!("{path}/status");
    let file: File = fs::openat2(
        &proc,
        status.as_str(),
        OFlags::RDONLY | SAFE | OFlags::NONBLOCK,
        Mode::empty(),
        INSIDE,
    )?
    .into();
    let mut data = String::new();
    file.take(65_537).read_to_string(&mut data)?;
    let caps: Vec<_> = data
        .lines()
        .filter_map(|line| line.strip_prefix("CapEff:"))
        .collect();
    if data.len() > 65_536 || caps.len() != 1 || u64::from_str_radix(caps[0].trim(), 16)? != 0 {
        return Err(invalid(
            "use an ordinary Linux thread with no effective capabilities",
        ));
    }
    Ok(uid)
}

pub(crate) fn literal(path: &Path) -> Result<&str> {
    let text = path
        .to_str()
        .ok_or_else(|| invalid("workspace paths require exact UTF-8"))?;
    if !text.starts_with('/')
        || text == "/"
        || text.len() > 4096
        || text[1..]
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == ".." || p.chars().any(char::is_control))
        || matches!(text[1..].split('/').next(), Some("dev" | "sys" | "proc"))
    {
        return Err(invalid(
            "select an exact absolute ordinary path, not a device, alias or pipe",
        ));
    }
    Ok(text)
}

fn plain_relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || !path.components().all(|p| matches!(p, Component::Normal(_)))
    {
        return Err(invalid(
            "not a literal path relative to the selected directory",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Stamp {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) length: u64,
    mode: u32,
    links: u64,
    uid: u32,
    modified: (i64, i64),
    change: (i64, i64),
}

pub(crate) fn stamp(m: &Metadata) -> Stamp {
    Stamp {
        dev: m.dev(),
        ino: m.ino(),
        length: m.len(),
        mode: m.mode(),
        links: m.nlink(),
        uid: m.uid(),
        modified: (m.mtime(), m.mtime_nsec()),
        change: (m.ctime(), m.ctime_nsec()),
    }
}

pub(crate) fn measure(file: &File, mut copy: impl Write) -> Result<Body> {
    let before = stamp(&file.metadata()?);
    if before.length > i64::MAX as u64 {
        return Err(invalid("file is too large for local native I/O"));
    }
    let mut bytes = vec![0; CHUNK];
    let mut sha = Sha256::new();
    let mut at = 0;
    while at < before.length {
        let n = usize::try_from((before.length - at).min(CHUNK as u64))?;
        let read = file.read_at(&mut bytes[..n], at)?;
        if read == 0 {
            return Err(invalid("the selected complete file ended early"));
        }
        sha.update(&bytes[..read]);
        copy.write_all(&bytes[..read])?;
        at += read as u64;
    }
    if file.read_at(&mut bytes[..1], at)? != 0 || stamp(&file.metadata()?) != before {
        return Err(invalid("the original complete file changed during read"));
    }
    Ok(Body {
        length: at,
        sha256: Digest::parse(&hex(&sha.finalize()))?,
    })
}

fn proc_root() -> Result<(File, String)> {
    let proc: File = fs::openat2(
        CWD,
        "/proc",
        OFlags::PATH | OFlags::DIRECTORY | SAFE,
        Mode::empty(),
        WALK,
    )?
    .into();
    if fs::fstatfs(&proc)?.f_type != 0x9fa0 {
        return Err(invalid("not a genuine native Linux procfs"));
    }
    let pid = std::process::id().to_string();
    if fs::readlinkat(&proc, "self", Vec::new())?.to_bytes() != pid.as_bytes() {
        return Err(invalid("the genuine procfs does not identify this process"));
    }
    Ok((proc, pid))
}

fn proc_fds() -> Result<File> {
    let (proc, pid) = proc_root()?;
    let path = format!("{pid}/fd");
    Ok(fs::openat2(
        &proc,
        path.as_str(),
        OFlags::PATH | OFlags::DIRECTORY | SAFE,
        Mode::empty(),
        INSIDE,
    )?
    .into())
}

// Access the held inode, not its untrusted name. An O_PATH file descriptor is still
// live throughout the procfs reopen; a rename cannot redirect it to a special file.
fn reopen_regular(pin: &File, before: Stamp, access: OFlags) -> Result<File> {
    if before.mode & 0o170000 != 0o100000 || stamp(&pin.metadata()?) != before {
        return Err(invalid("the held regular inode changed before access"));
    }
    let proc = proc_fds()?;
    let number = pin.as_raw_fd().to_string();
    if fs::statat(&proc, number.as_str(), AtFlags::SYMLINK_NOFOLLOW)?.st_mode & 0o170000 != 0o120000
    {
        return Err(invalid(
            "not the genuine procfs link for the held regular inode",
        ));
    }
    let file: File = fs::openat(
        &proc,
        number.as_str(),
        access | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )?
    .into();
    if stamp(&file.metadata()?) != before {
        return Err(invalid(
            "access did not reopen the exact held regular inode",
        ));
    }
    Ok(file)
}

fn local_volume(fd: &File) -> Result<()> {
    if !matches!(fs::fstatfs(fd)?.f_type, 0xef53 | 0x5846_5342) {
        return Err(invalid(
            "only actual local ext4 and XFS are admitted in this Linux candidate",
        ));
    }
    Ok(())
}

pub(crate) struct Folder {
    path: PathBuf,
    fd: File,
    identity: (u64, u64),
    uid: u32,
    private: bool,
}

impl Folder {
    pub(crate) fn open(path: &Path, private: bool) -> Result<Self> {
        literal(path)?;
        let uid = ordinary_uid()?;
        let fd: File = fs::openat2(
            CWD,
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | SAFE,
            Mode::empty(),
            WALK,
        )?
        .into();
        let m = fd.metadata()?;
        let out = Self {
            path: path.to_owned(),
            fd,
            uid,
            identity: (m.dev(), m.ino()),
            private,
        };
        out.verify(&m)?;
        local_volume(&out.fd)?;
        Ok(out)
    }

    fn verify(&self, m: &Metadata) -> Result<()> {
        if !m.is_dir()
            || (m.dev(), m.ino()) != self.identity
            || m.uid() != self.uid
            || m.mode() & 0o022 != 0
            || (self.private && m.mode() & 0o777 != 0o700)
        {
            return Err(invalid(
                "the original directory was replaced or does not have qualified ownership/mode",
            ));
        }
        Ok(())
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn check(&self) -> Result<()> {
        self.verify(&self.fd.metadata()?)?;
        let named: File = fs::openat2(
            CWD,
            &self.path,
            OFlags::PATH | OFlags::DIRECTORY | SAFE,
            Mode::empty(),
            WALK,
        )?
        .into();
        self.verify(&named.metadata()?)?;
        local_volume(&named)?;
        Ok(())
    }

    pub(crate) fn names(&self, relative: &Path) -> Result<Vec<OsString>> {
        let dir = if relative.as_os_str().is_empty() {
            self.fd.try_clone()?
        } else {
            plain_relative(relative)?;
            fs::openat2(
                &self.fd,
                relative,
                OFlags::RDONLY | OFlags::DIRECTORY | SAFE,
                Mode::empty(),
                INSIDE,
            )?
            .into()
        };
        let actual = dir.metadata()?;
        if actual.uid() != self.uid || actual.mode() & 0o022 != 0 {
            return Err(invalid(
                "directory under the original root changed protection",
            ));
        }
        let mut rows = Vec::new();
        for entry in Dir::read_from(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            if rows.len() >= 16_384 {
                return Err(invalid("directory visit budget exceeded"));
            }
            rows.push(OsString::from_vec(name.to_vec()));
        }
        self.check()?;
        Ok(rows)
    }

    pub(crate) fn pin(&self, path: &Path) -> Result<File> {
        plain_relative(path)?;
        Ok(fs::openat2(&self.fd, path, OFlags::PATH | SAFE, Mode::empty(), INSIDE)?.into())
    }

    fn retain(&self) -> Result<Self> {
        Ok(Self {
            path: self.path.clone(),
            fd: self.fd.try_clone()?,
            identity: self.identity,
            uid: self.uid,
            private: self.private,
        })
    }

    pub(crate) fn source(&self, path: &Path) -> Result<Source> {
        self.check()?;
        let pin = self.pin(path)?;
        let m = pin.metadata()?;
        if !m.is_file() || m.uid() != self.uid || m.nlink() != 1 || m.mode() & 0o022 != 0 {
            return Err(invalid(
                "the selected inode is not an owned ordinary single-link file",
            ));
        }
        let before = stamp(&m);
        let file = reopen_regular(&pin, before, OFlags::RDONLY)?;
        let source = Source {
            folder: self.retain()?,
            path: path.to_owned(),
            pin,
            file,
            before,
        };
        source.check()?;
        Ok(source)
    }

    pub(crate) fn absent(&self, name: &OsStr) -> Result<()> {
        self.check()?;
        plain_relative(Path::new(name))?;
        match self.pin(Path::new(name)) {
            Err(e) if e.downcast_ref::<rustix::io::Errno>() == Some(&rustix::io::Errno::NOENT) => {
                Ok(())
            }
            Ok(_) => Err(invalid("the selected local name already exists")),
            Err(e) => Err(e),
        }
    }

    pub(crate) fn new_private(&self, name: &str) -> Result<File> {
        self.check()?;
        plain_relative(Path::new(name))?;
        Ok(fs::openat2(
            &self.fd,
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | SAFE,
            Mode::RUSR | Mode::WUSR,
            INSIDE,
        )?
        .into())
    }

    pub(crate) fn sync(&self) -> Result<()> {
        self.check()?;
        self.fd.sync_all()?;
        self.check()
    }

    pub(crate) fn unlinked(&self) -> Result<File> {
        self.check()?;
        Ok(fs::openat(
            &self.fd,
            ".",
            OFlags::RDWR | OFlags::TMPFILE | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?
        .into())
    }

    pub(crate) fn publish(&self, name: &OsStr, file: &File, expected: &Body) -> Result<()> {
        self.publish_guarded(name, file, expected, || Ok(()))
    }

    pub(crate) fn publish_guarded(
        &self,
        name: &OsStr,
        file: &File,
        expected: &Body,
        before_link: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        self.absent(name)?;
        file.sync_all()?;
        if &measure(file, std::io::sink())? != expected {
            return Err(invalid("output readback did not match original bytes"));
        }
        let before = stamp(&file.metadata()?);
        if before.links != 0
            || before.uid != self.uid
            || before.dev != self.identity.0
            || before.mode & 0o170000 != 0o100000
            || !(before.mode & 0o777 == 0o600 || (self.private && before.mode & 0o777 == 0o400))
        {
            return Err(invalid("output is not the original private unnamed inode"));
        }
        self.link_unpublished(name, file, before_link)?;
        // The name may exist if any check from here onward errors. Do not retry with a replacement.
        let check = || -> Result<()> {
            self.sync()?;
            let named = self.source(Path::new(name))?;
            let published = named.before;
            if (
                published.dev,
                published.ino,
                published.length,
                published.modified,
            ) != (before.dev, before.ino, before.length, before.modified)
                || stamp(&file.metadata()?) != published
            {
                return Err(invalid("the original published identity changed"));
            }
            Ok(())
        };
        check().map_err(|_| invalid("verified output may already have been published; check original name, never replace"))
    }

    // The last name insertion must not rely on the earlier absence check.
    fn link_unpublished(
        &self,
        name: &OsStr,
        file: &File,
        before_link: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let proc = proc_fds()?;
        let number = file.as_raw_fd().to_string();
        self.check()?;
        before_link()?;
        self.check()?;
        fs::linkat(
            &proc,
            number.as_str(),
            &self.fd,
            name,
            AtFlags::SYMLINK_FOLLOW,
        )?;
        Ok(())
    }

    pub(crate) fn remove_verified(
        &self,
        name: &str,
        expected: &Body,
        original: Stamp,
        before_unlink: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let source = self.source(Path::new(name))?;
        if source.before.dev != original.dev
            || source.before.ino != original.ino
            || source.before != original
            || &source.scan(std::io::sink())? != expected
        {
            return Err(invalid("not the original completed transfer payload"));
        }
        source.check()?;
        before_unlink()?;
        source.check()?;
        fs::unlinkat(&self.fd, name, AtFlags::empty())?;
        self.sync()
    }

    fn lease_stamp(&self, fd: &File) -> Result<Stamp> {
        let held = fd.metadata()?;
        if !held.is_file()
            || held.dev() != self.identity.0
            || held.uid() != self.uid
            || held.nlink() != 1
            || held.mode() & 0o7777 != 0o600
        {
            return Err(invalid("workspace lock must be the private ordinary file"));
        }
        Ok(stamp(&held))
    }

    pub(crate) fn lock(&self, name: &str) -> Result<Lease> {
        let path = Path::new(name);
        if !self.private || path.file_name() != Some(OsStr::new(name)) {
            return Err(invalid("workspace lock requires one private local name"));
        }
        self.check()?;
        let existing = |pin: File| -> Result<(File, Stamp)> {
            let before = self.lease_stamp(&pin)?;
            Ok((reopen_regular(&pin, before, OFlags::RDWR)?, before))
        };
        let (file, before) = match self.pin(path) {
            Ok(pin) => existing(pin)?,
            Err(e) if e.downcast_ref::<Errno>() == Some(&Errno::NOENT) => {
                // An absent name is created only with O_EXCL; a competing new name is pinned first.
                match self.new_private(name) {
                    Ok(file) => {
                        let before = self.lease_stamp(&file)?;
                        (file, before)
                    }
                    Err(e) if e.downcast_ref::<Errno>() == Some(&Errno::EXIST) => {
                        existing(self.pin(path)?)?
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(e) => return Err(e),
        };
        fs::flock(&file, fs::FlockOperation::NonBlockingLockExclusive)?;
        let lease = Lease {
            folder: self.retain()?,
            name: path.to_owned(),
            file,
            before,
        };
        lease.check()?;
        Ok(lease)
    }
}

// Retain the flock and its selected name for the full operation, not merely
// until open returns. This is cooperative exclusion in the private root.
pub(crate) struct Lease {
    folder: Folder,
    name: PathBuf,
    file: File,
    before: Stamp,
}

impl Lease {
    pub(crate) fn check(&self) -> Result<()> {
        self.folder.check()?;
        if self.folder.lease_stamp(&self.file).ok() != Some(self.before)
            || self
                .folder
                .pin(&self.name)
                .and_then(|pin| self.folder.lease_stamp(&pin))
                .ok()
                != Some(self.before)
        {
            return Err(invalid(
                "the original private workspace lease changed or was detached",
            ));
        }
        Ok(())
    }
}

pub(crate) struct Source {
    folder: Folder,
    path: PathBuf,
    pin: File,
    pub(crate) file: File,
    pub(crate) before: Stamp,
}

impl Source {
    pub(crate) fn check(&self) -> Result<()> {
        self.folder.check()?;
        if stamp(&self.pin.metadata()?) != self.before
            || stamp(&self.file.metadata()?) != self.before
            || stamp(&self.folder.pin(&self.path)?.metadata()?) != self.before
        {
            return Err(invalid("the exact original named file changed"));
        }
        Ok(())
    }

    pub(crate) fn scan(&self, target: impl Write) -> Result<Body> {
        self.check()?;
        let body = measure(&self.file, target)?;
        self.check()?;
        Ok(body)
    }
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
