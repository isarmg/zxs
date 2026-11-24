//! An enrolled pool, held under the original state directory's exclusive lock.
//! Every operation on an existing roster starts here; host observation never does.
use super::{
    DESCRIPTOR, FIXED_ROOTS, FORMAT, MAX_DATA, PoolIdentity, SelectedRoles, login_ok, random,
    refused, require_held,
};
use crate::pinned::Entity;
use rustix::fs::{FlockOperation, Mode, OFlags, RawDir, ResolveFlags, flock, openat, openat2};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, Metadata},
    io::{self, Read},
    mem::MaybeUninit,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
};
use zeroize::Zeroizing;

mod add_data;
mod disk_move;
pub(crate) use disk_move::DiskMove;
pub(super) mod import_command;
mod import_one;
mod init_data;
pub(crate) use import_one::batch::ImportBatch;
mod installed;
mod offline_catalog;
mod password;
mod retire_data;
mod stage;
mod verify;
pub(crate) use verify::OfflineVerify;

const EPOCH: usize = FORMAT.len() + 16;
const SALT: usize = EPOCH + 16;
const TAG: usize = SALT + 16;
const LOGIN: usize = TAG + 32;
const MAX_DESCRIPTOR: usize = LOGIN + 1 + 64 + 1 + (FIXED_ROOTS + MAX_DATA) * (2 + 4095 + 32);
const SCOPED: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_XDEV);

pub(crate) struct OfflineSession {
    selected: SelectedRoles,
    locked: File,
    original: Record,
    identity: PoolIdentity,
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

fn is_other_stage(name: &[u8], allowed: Option<&str>) -> bool {
    name.starts_with(b".server-stage-") && allowed.map(str::as_bytes) != Some(name)
}

fn require_no_stage(dir: &File, allowed_stage: Option<&str>) -> io::Result<()> {
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
        if is_other_stage(name, allowed_stage) {
            return Err(io::Error::other(
                "an original descriptor stage remains; preserve it for offline inspection",
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

fn parse_id(text: &str) -> Option<[u8; 16]> {
    fn nibble(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        }
    }
    let raw: &[u8; 32] = text.as_bytes().try_into().ok()?;
    let mut id = [0; 16];
    let (pairs, _) = raw.as_chunks::<2>();
    for (value, two) in id.iter_mut().zip(pairs) {
        *value = (nibble(two[0])? << 4) | nibble(two[1])?;
    }
    (id != [0; 16]).then_some(id)
}

fn live_role_binding(
    domain: &[u8],
    descriptor: &[u8; 32],
    roles: &[[u64; 3]],
) -> io::Result<[u8; 32]> {
    if !(3..=64).contains(&roles.len()) {
        return Err(refused());
    }
    let count = u8::try_from(roles.len()).map_err(|_| refused())?;
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(descriptor);
    hash.update([count]);
    for &[device, inode, mount] in roles {
        if inode == 0 || mount == 0 {
            return Err(refused());
        }
        hash.update(device.to_be_bytes());
        hash.update(inode.to_be_bytes());
        hash.update(mount.to_be_bytes());
    }
    Ok(hash.finalize().into())
}

impl SelectedRoles {
    /// Acquire the same state EX used by fresh provisioning. Core's shared
    /// lease makes this fail while a native service is still active/draining.
    fn open_offline(self, allowed_stage: Option<&str>) -> io::Result<OfflineSession> {
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
        require_no_stage(&locked, allowed_stage)?;
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
        let out = OfflineSession {
            selected: self,
            identity: PoolIdentity { pool, tokens },
            locked,
            original,
            login,
            epoch,
        };
        out.require_original()?;
        Ok(out)
    }

    pub(crate) fn open_durable(self) -> io::Result<OfflineSession> {
        let out = self.open_offline(None)?;
        // Establish the current descriptor's durability after any earlier
        // stopped writer with a visible name. This is not credential rotation.
        out.original.held.sync_all()?;
        out.locked.sync_all()?;
        out.require_original()?;
        Ok(out)
    }
    /// Deliver the complete census and optional rows under the offline lease.
    pub(super) fn inspect_import_source<T>(
        self,
        path: &Path,
        mut preview: Option<&mut Vec<u8>>,
        check_target: bool,
        report: impl FnOnce(super::import_preflight::Counts, Option<&[u8]>) -> io::Result<T>,
    ) -> io::Result<T> {
        let selected = self.open_offline(None)?;
        let names = check_target
            .then(|| offline_catalog::read_names(&selected))
            .transpose()?;
        let result = super::import_preflight::scan(
            &selected.selected,
            path,
            preview.as_deref_mut(),
            names.as_deref(),
            || selected.require_original(),
        )?;
        report(result, preview.as_deref().map(Vec::as_slice))
    }
}

impl OfflineSession {
    fn role_binding(&self, domain: &[u8]) -> io::Result<[u8; 32]> {
        // The original roster is already checked against the protected descriptor.
        // Hash the exact first held roots, not a replacement reopened by name.
        let roles = self
            .selected
            .roots
            .iter()
            .map(|root| {
                let (entity, mount) = require_held(&root.input(), self.selected.uid)?;
                Ok([entity.dev, entity.ino, mount.id])
            })
            .collect::<io::Result<Vec<_>>>()?;
        let descriptor = Sha256::digest(self.original.full.as_slice()).into();
        live_role_binding(domain, &descriptor, &roles)
    }

    fn data_token(&self, bytes: &[u8]) -> io::Result<[u8; 32]> {
        self.selected
            .roots
            .iter()
            .zip(&self.identity.tokens)
            .skip(FIXED_ROOTS)
            .find(|(root, _)| root.original.path().as_os_str().as_bytes() == bytes)
            .map(|(_, token)| *token)
            .ok_or_else(refused)
    }

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
        self.selected
            .issue(&self.login, &self.identity)?
            .require_roots(true)?;
        let now = read(&self.locked, DESCRIPTOR, self.selected.uid)?;
        if stamp(&now.meta) != stamp(&self.original.meta)
            || stamp(&self.original.held.metadata()?) != stamp(&now.meta)
            || now.full.as_slice() != self.original.full.as_slice()
        {
            return Err(refused());
        }
        Ok(())
    }
}
