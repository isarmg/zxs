//! An operator-retained single-file ticket and the original external source.
use super::{OfflineSession, refused};
use crate::{
    headless_setup::import_preflight::{SourceTree, Stamp, pin_source, same_name_candidate},
    path_rules::Kind,
    pinned::{Entity, Mount, Pinned, external_import_reader, require_no_automount},
};
use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read},
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
};

pub(super) mod batch;
mod ipc;

pub(super) use ipc::{ApplyOutcome, apply};

const MAGIC: &[u8; 8] = b"ZXIMP1\0\0";
const PATH_BYTES: usize = 4095;
pub(super) const TICKET_BYTES: usize = 8 + 16 + 16 + 32 + 3 * (2 + PATH_BYTES) + 104 + 2 + 255;
const SCOPED: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_XDEV);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Witness {
    entity: Entity,
    uid: u32,
    mode: u32,
    links: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    length: u64,
}

impl Witness {
    fn read(file: &File, mount: Mount) -> io::Result<Self> {
        require_no_automount(file)?;
        let m = file.metadata()?;
        // Mount-root status describes an individual inode, not mount identity:
        // an ordinary file in a selected mount root is not itself that root.
        if !m.is_file() || m.nlink() == 0 || Mount::on(file)?.is_none_or(|now| now.id != mount.id) {
            return Err(refused());
        }
        Ok(Self {
            entity: Entity::of(&m)?,
            uid: m.uid(),
            mode: m.mode(),
            links: m.nlink(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
            length: m.len(),
        })
    }
}

pub(super) struct ExternalSource {
    root: Pinned,
    directory: File,
    relative: Vec<u8>,
    mount: Mount,
    held: File,
    witness: Witness,
}

fn entry(parent: &File, relative: &[u8]) -> io::Result<File> {
    Ok(File::from(openat2(
        parent,
        OsStr::from_bytes(relative),
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        SCOPED,
    )?))
}

impl ExternalSource {
    fn select(session: &OfflineSession, source: &Path, relative: &[u8]) -> io::Result<Self> {
        if !relative_path(relative) {
            return Err(refused());
        }
        let root = pin_source(&session.selected, source)?;
        let mount = root.require_same_named_mount()?;
        session.selected.scope.full_local_mount(mount.id)?;
        let directory = root.storage_directory()?;
        let held = entry(&directory, relative)?;
        let witness = Witness::read(&held, mount)?;
        let source = Self {
            root,
            directory,
            relative: relative.to_vec(),
            mount,
            held,
            witness,
        };
        source.verify()?;
        Ok(source)
    }

    pub(super) fn verify(&self) -> io::Result<()> {
        if self.root.require_same_named_mount()? != self.mount
            || Mount::on(&self.directory)? != Some(self.mount)
            || Entity::of(&self.directory.metadata()?)? != self.root.entity()
            || Witness::read(&self.held, self.mount)? != self.witness
            || Witness::read(&entry(&self.directory, &self.relative)?, self.mount)? != self.witness
        {
            return Err(refused());
        }
        Ok(())
    }

    pub(super) fn reader(&self) -> io::Result<File> {
        self.verify()?;
        let file = external_import_reader(&self.held, self.witness.entity, self.mount.id)?;
        if Witness::read(&file, self.mount)? != self.witness {
            return Err(refused());
        }
        self.verify()?;
        Ok(file)
    }

    fn fingerprint(&self) -> io::Result<[u8; 32]> {
        let mut file = self.reader()?;
        let mut digest = Sha256::new();
        let mut buffer = vec![0; 64 * 1024];
        let mut remaining = self.witness.length;
        while remaining != 0 {
            let width =
                usize::try_from(remaining.min(buffer.len() as u64)).map_err(|_| refused())?;
            file.read_exact(&mut buffer[..width])?;
            digest.update(&buffer[..width]);
            remaining -= width as u64;
        }
        match file.read_exact(&mut buffer[..1]) {
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {}
            Err(error) => return Err(error),
            Ok(()) => return Err(refused()),
        }
        if Witness::read(&file, self.mount)? != self.witness {
            return Err(refused());
        }
        self.verify()?;
        Ok(digest.finalize().into())
    }
}

fn relative_path(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes.len() <= PATH_BYTES
        && !bytes.contains(&0)
        && bytes
            .split(|&byte| byte == b'/')
            .all(|part| !part.is_empty() && !matches!(part, b"." | b".."))
}

pub(super) struct Ticket {
    seed: [u8; 16],
    pool: [u8; 16],
    token: [u8; 32],
    target: Vec<u8>,
    source_root: Vec<u8>,
    relative: Vec<u8>,
    source: Witness,
    sha256: [u8; 32],
    name: Vec<u8>,
}

impl Ticket {
    pub(super) fn prepare(
        session: &OfflineSession,
        target: &Path,
        source: &Path,
        relative: &[u8],
        name: &[u8],
    ) -> io::Result<Self> {
        Self::prepare_observed(session, target, source, relative, name, None)
    }

    fn prepare_observed(
        session: &OfflineSession,
        target: &Path,
        source: &Path,
        relative: &[u8],
        name: &[u8],
        observation: Option<(&SourceTree, &Stamp)>,
    ) -> io::Result<Self> {
        if !same_name_candidate(name) {
            return Err(refused());
        }
        session.require_original()?;
        let token = session.data_token(target.as_os_str().as_bytes())?;
        let origin = ExternalSource::select(session, source, relative)?;
        let confirm_observation = || {
            if let Some((tree, stamp)) = observation {
                tree.require_file(&origin.root, origin.mount, &origin.held, stamp)?;
            }
            Ok::<(), io::Error>(())
        };
        confirm_observation()?;
        let sha256 = origin.fingerprint()?;
        confirm_observation()?;
        session.require_original()?;
        let seed = super::random::<16>()?;
        let ticket = Self {
            seed,
            pool: session.identity.pool,
            token,
            target: target.as_os_str().as_bytes().to_vec(),
            source_root: source.as_os_str().as_bytes().to_vec(),
            relative: relative.to_vec(),
            source: origin.witness,
            sha256,
            name: name.to_vec(),
        };
        ticket.validate()?;
        Ok(ticket)
    }

    pub(super) fn require_target(&self, session: &OfflineSession) -> io::Result<()> {
        session.require_original()?;
        if self.pool != session.identity.pool || session.data_token(&self.target)? != self.token {
            return Err(refused());
        }
        Ok(())
    }

    /// Call only after checking for the original prior receipt.
    pub(super) fn source(&self, session: &OfflineSession) -> io::Result<ExternalSource> {
        self.require_target(session)?;
        let original = ExternalSource::select(
            session,
            Path::new(OsStr::from_bytes(&self.source_root)),
            &self.relative,
        )?;
        if original.witness != self.source {
            return Err(refused());
        }
        Ok(original)
    }

    pub(super) fn encode(&self) -> io::Result<Vec<u8>> {
        self.validate()?;
        let mut out = Vec::with_capacity(TICKET_BYTES);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.seed);
        out.extend_from_slice(&self.pool);
        out.extend_from_slice(&self.token);
        field(&mut out, &self.target)?;
        field(&mut out, &self.source_root)?;
        field(&mut out, &self.relative)?;
        let source = &self.source;
        out.extend_from_slice(&source.entity.dev.to_be_bytes());
        out.extend_from_slice(&source.entity.ino.to_be_bytes());
        out.extend_from_slice(&source.uid.to_be_bytes());
        out.extend_from_slice(&source.mode.to_be_bytes());
        out.extend_from_slice(&source.links.to_be_bytes());
        for (seconds, nanos) in [source.modified, source.changed] {
            out.extend_from_slice(&seconds.to_be_bytes());
            out.extend_from_slice(&nanos.to_be_bytes());
        }
        out.extend_from_slice(&source.length.to_be_bytes());
        out.extend_from_slice(&self.sha256);
        field(&mut out, &self.name)?;
        Ok(out)
    }

    pub(super) fn decode(raw: &[u8]) -> io::Result<Self> {
        if raw.len() > TICKET_BYTES {
            return Err(refused());
        }
        let mut bytes = raw.strip_prefix(MAGIC).ok_or_else(refused)?;
        let seed = fixed(&mut bytes)?;
        let pool = fixed(&mut bytes)?;
        let token = fixed(&mut bytes)?;
        let target = variable(&mut bytes, PATH_BYTES)?.to_vec();
        let source_root = variable(&mut bytes, PATH_BYTES)?.to_vec();
        let relative = variable(&mut bytes, PATH_BYTES)?.to_vec();
        let source = Witness {
            entity: Entity {
                dev: u64::from_be_bytes(fixed(&mut bytes)?),
                ino: u64::from_be_bytes(fixed(&mut bytes)?),
            },
            uid: u32::from_be_bytes(fixed(&mut bytes)?),
            mode: u32::from_be_bytes(fixed(&mut bytes)?),
            links: u64::from_be_bytes(fixed(&mut bytes)?),
            modified: pair(&mut bytes)?,
            changed: pair(&mut bytes)?,
            length: u64::from_be_bytes(fixed(&mut bytes)?),
        };
        let sha256 = fixed(&mut bytes)?;
        let name = variable(&mut bytes, 255)?.to_vec();
        if !bytes.is_empty() {
            return Err(refused());
        }
        let ticket = Self {
            seed,
            pool,
            token,
            target,
            source_root,
            relative,
            source,
            sha256,
            name,
        };
        ticket.validate()?;
        Ok(ticket)
    }

    fn validate(&self) -> io::Result<()> {
        Kind::StorageRoot
            .check_path(&self.target)
            .map_err(crate::pinned::invalid)?;
        Kind::StorageRoot
            .check_path(&self.source_root)
            .map_err(crate::pinned::invalid)?;
        if self.seed == [0; 16]
            || self.pool == [0; 16]
            || self.token == [0; 32]
            || self.target.len() > PATH_BYTES
            || self.source_root.len() > PATH_BYTES
            || !relative_path(&self.relative)
            || !same_name_candidate(&self.name)
            || self.source.entity.ino == 0
            || self.source.mode & 0o170_000 != 0o100_000
            || self.source.links == 0
            || !(0..1_000_000_000).contains(&self.source.modified.1)
            || !(0..1_000_000_000).contains(&self.source.changed.1)
        {
            return Err(refused());
        }
        Ok(())
    }
}

fn field(out: &mut Vec<u8>, value: &[u8]) -> io::Result<()> {
    out.extend_from_slice(
        &u16::try_from(value.len())
            .map_err(|_| refused())?
            .to_be_bytes(),
    );
    out.extend_from_slice(value);
    Ok(())
}
fn variable<'a>(bytes: &mut &'a [u8], limit: usize) -> io::Result<&'a [u8]> {
    let width = usize::from(u16::from_be_bytes(fixed(bytes)?));
    if width > limit {
        return Err(refused());
    }
    let (part, tail) = bytes.split_at_checked(width).ok_or_else(refused)?;
    *bytes = tail;
    Ok(part)
}
fn fixed<const N: usize>(bytes: &mut &[u8]) -> io::Result<[u8; N]> {
    let (part, tail) = bytes.split_at_checked(N).ok_or_else(refused)?;
    *bytes = tail;
    part.try_into().map_err(|_| refused())
}
fn pair(bytes: &mut &[u8]) -> io::Result<(i64, i64)> {
    Ok((
        i64::from_be_bytes(fixed(bytes)?),
        i64::from_be_bytes(fixed(bytes)?),
    ))
}

#[cfg(test)]
#[path = "../../../tests/maintenance/import_one_ticket.rs"]
mod tests;
