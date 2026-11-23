//! Offline replacement of the one current native account verifier.
use super::{
    DESCRIPTOR, FIXED_ROOTS, FORMAT, FreshSelection, IssueError, MAX_DATA, fresh_verifier,
    login_ok, random, refused, require_held, stage_name,
};
use crate::{fresh_password, pinned::Entity};
use rustix::fs::{
    FlockOperation, Mode, OFlags, RawDir, ResolveFlags, fchmod, flock, openat, openat2, renameat,
};
use std::{
    fs::{File, Metadata},
    io::{self, Read, Write},
    mem::MaybeUninit,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

#[path = "headless_add_data.rs"]
mod add_data;

const EPOCH: usize = FORMAT.len() + 16;
const SALT: usize = EPOCH + 16;
const TAG: usize = SALT + 16;
const LOGIN: usize = TAG + 32;
const MAX_DESCRIPTOR: usize = LOGIN + 1 + 64 + 1 + (FIXED_ROOTS + MAX_DATA) * (2 + 4095 + 32);
const SCOPED: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_XDEV);

pub(crate) struct Reset {
    selected: FreshSelection,
    locked: File,
    original: Record,
    login: String,
    epoch: [u8; 16],
}

struct Record {
    held: File,
    meta: Metadata,
    full: Zeroizing<Vec<u8>>,
}

fn stamp(m: &Metadata) -> (u64, u64, u32, u32, u64, u64, i64, i64, i64, i64) {
    (
        m.dev(),
        m.ino(),
        m.uid(),
        m.mode(),
        m.nlink(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}

fn read(dir: &File, name: &str, uid: u32) -> io::Result<Record> {
    let held = File::from(openat2(
        dir,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK | OFlags::NOATIME,
        Mode::empty(),
        SCOPED,
    )?);
    let meta = held.metadata()?;
    if !meta.is_file()
        || meta.uid() != uid
        || meta.mode() & 0o7777 != 0o600
        || meta.nlink() != 1
        || meta.ino() == 0
        || meta.len() > MAX_DESCRIPTOR as u64
    {
        return Err(refused());
    }
    let mut full = Zeroizing::new(Vec::new());
    (&held)
        .take(MAX_DESCRIPTOR as u64 + 1)
        .read_to_end(&mut full)?;
    if full.len() as u64 != meta.len() || stamp(&held.metadata()?) != stamp(&meta) {
        return Err(refused());
    }
    Ok(Record { held, meta, full })
}

fn require_no_stage(dir: &File, resume_add: bool) -> io::Result<()> {
    let reader = File::from(openat2(
        dir,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOATIME | OFlags::CLOEXEC,
        Mode::empty(),
        SCOPED,
    )?);
    let mut buf = [MaybeUninit::uninit(); 4096];
    let mut rows = RawDir::new(&reader, &mut buf);
    while let Some(row) = rows.next() {
        let row = row?;
        let name = row.file_name().to_bytes();
        if name.starts_with(b".server-stage-") && !(resume_add && name == b".server-stage-add-data") {
            return Err(io::Error::other(
                "an original credential stage remains; preserve it for offline inspection",
            ));
        }
    }
    Ok(())
}

fn field<'a>(raw: &mut &'a [u8], len: usize) -> io::Result<&'a [u8]> {
    let (part, rest) = raw.split_at_checked(len).ok_or_else(refused)?;
    *raw = rest;
    Ok(part)
}

fn fixed<const N: usize>(raw: &mut &[u8]) -> io::Result<[u8; N]> {
    field(raw, N)?.try_into().map_err(|_| refused())
}

impl FreshSelection {
    /// Acquire the same state EX used by fresh provisioning. Core's shared
    /// lease makes this fail while a native service is still active/draining.
    fn open_offline(mut self, resume_add: bool) -> io::Result<Reset> {
        self.require_site()?;
        let locked = File::from(openat(
            &self.state.directory,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        flock(&locked, FlockOperation::NonBlockingLockExclusive)?;
        if Entity::of(&locked.metadata()?)? != require_held(&self.state.input(), self.uid)?.0 {
            return Err(refused());
        }
        require_no_stage(&locked, resume_add)?;
        let original = read(&locked, DESCRIPTOR, self.uid)?;
        let mut body = original.full.strip_prefix(FORMAT).ok_or_else(refused)?;
        let pool = fixed::<16>(&mut body)?;
        let epoch = fixed::<16>(&mut body)?;
        let salt = fixed::<16>(&mut body)?;
        let tag = Zeroizing::new(fixed::<32>(&mut body)?);
        let [len] = fixed(&mut body)?;
        let login =
            std::str::from_utf8(field(&mut body, usize::from(len))?).map_err(|_| refused())?;
        if !login_ok(login)
            || pool == [0; 16]
            || epoch == [0; 16]
            || pool == epoch
            || salt == [0; 16]
            || *tag == [0; 32]
        {
            return Err(refused());
        }
        let login = login.to_owned();
        let [count] = fixed(&mut body)?;
        if usize::from(count) + FIXED_ROOTS != self.roots.len() {
            return Err(refused());
        }
        let mut tokens = vec![[0; 32]; self.roots.len()];
        for (root, token) in self.roots.iter().zip(&mut tokens) {
            let width = usize::from(u16::from_be_bytes(fixed(&mut body)?));
            if width > 4095
                || field(&mut body, width)? != root.original.path().as_os_str().as_bytes()
            {
                return Err(refused());
            }
            *token = fixed(&mut body)?;
        }
        if !body.is_empty() {
            return Err(refused());
        }
        self.pool = pool;
        self.tokens = tokens;
        let out = Reset {
            selected: self,
            locked,
            original,
            login,
            epoch,
        };
        out.require_original()?;
        Ok(out)
    }

    pub(crate) fn begin_reset(self) -> io::Result<Reset> {
        let out = self.open_offline(false)?;
        // Establish the current descriptor's durability after any earlier
        // stopped writer with a visible name. This is not credential rotation.
        out.original.held.sync_all()?;
        out.locked.sync_all()?;
        out.require_original()?;
        Ok(out)
    }
    /// The callback receives census counts only while the current offline
    /// lease is held. No sync, credential write or mutable Reset escapes.
    pub(super) fn inspect_import_source<T>(
        self,
        path: &Path,
        report: impl FnOnce(super::import_preflight::Counts) -> io::Result<T>,
    ) -> io::Result<T> {
        let selected = self.open_offline(false)?;
        let result = super::import_preflight::scan(&selected.selected, path, || {
            selected.require_original()
        })?;
        report(result)
    }
}

impl Reset {
    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.selected.paths()
    }
    pub(crate) fn login(&self) -> &str {
        &self.login
    }

    fn require_original(&self) -> io::Result<()> {
        self.selected.require_site()?;
        if Entity::of(&self.locked.metadata()?)?
            != require_held(&self.selected.state.input(), self.selected.uid)?.0
        {
            return Err(refused());
        }
        self.selected.issue(&self.login).require_roots(true)?;
        let now = read(&self.locked, DESCRIPTOR, self.selected.uid)?;
        if stamp(&now.meta) != stamp(&self.original.meta)
            || stamp(&self.original.held.metadata()?) != stamp(&now.meta)
            || now.full.as_slice() != self.original.full.as_slice()
        {
            return Err(refused());
        }
        Ok(())
    }

    pub(crate) fn apply(self, secret: &str, repeated: &str) -> Result<(), IssueError> {
        if !fresh_password::admissible(secret)
            || !fresh_password::admissible(repeated)
            || !bool::from(secret.as_bytes().ct_eq(repeated.as_bytes()))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the two valid account password entries must match",
            )
            .into());
        }
        self.require_original()?;
        let epoch = random::<16>()?;
        if epoch == self.epoch || epoch == self.selected.pool {
            return Err(refused().into());
        }
        let salt = random::<16>()?;
        let tag = fresh_verifier(secret, &salt)?;
        if *tag == [0; 32] {
            return Err(refused().into());
        }
        let mut updated = Zeroizing::new(self.original.full.to_vec());
        updated[EPOCH..EPOCH + 16].copy_from_slice(&epoch);
        updated[SALT..TAG].copy_from_slice(&salt);
        updated[TAG..LOGIN].copy_from_slice(&tag[..]);
        let stage = stage_name(&epoch);

        let commit = || -> io::Result<()> {
            let mut candidate = File::from(openat2(
                &self.locked,
                stage.as_str(),
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
                SCOPED,
            )?);
            fchmod(&candidate, Mode::RUSR | Mode::WUSR)?;
            candidate.write_all(&updated)?;
            candidate.sync_all()?;
            let prepared = read(&self.locked, &stage, self.selected.uid)?;
            let expected = stamp(&candidate.metadata()?);
            if stamp(&prepared.meta) != expected || prepared.full.as_slice() != updated.as_slice() {
                return Err(refused());
            }
            self.require_original()?;
            if stamp(&candidate.metadata()?) != expected
                || stamp(&read(&self.locked, &stage, self.selected.uid)?.meta) != expected
            {
                return Err(refused());
            }
            // One intentional overwrite of the proved current descriptor.
            // The state EX excludes Core/Helper adoption throughout; a name
            // changed outside the dedicated service owner is not supported.
            renameat(&self.locked, stage.as_str(), &self.locked, DESCRIPTOR)?;
            self.locked.sync_all()?;
            self.selected.require_site()?;
            self.selected.issue(&self.login).require_roots(true)?;
            let selected = read(&self.locked, DESCRIPTOR, self.selected.uid)?;
            if Entity::of(&selected.meta)? != Entity::of(&candidate.metadata()?)?
                || selected.full.as_slice() != updated.as_slice()
            {
                return Err(refused());
            }
            Ok(())
        };
        commit().map_err(IssueError::Uncertain)
    }
}
