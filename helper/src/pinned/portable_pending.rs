//! HeldPending observes only an explicitly selected SYSTEM; it never writes or syncs.
//! The separate private group step requires all original roots and a specific approved order.
mod group;
use super::{Entity, Kind, Mount, Pinned, ReaderBoundary, reopen_admitted_reader};
use crate::portable_claim::{LEN as CLAIM_LEN, ROOT_SLOT};
use crate::portable_genesis::{FLOOR_LEN, Initial, Invalid, MAX_GENESIS};
use rustix::fs::{FlockOperation, Mode, OFlags, RawDir, flock, openat};
use std::{
    ffi::OsStr, fs::File, io, mem::MaybeUninit,
    os::unix::{ffi::OsStrExt, fs::{FileExt, MetadataExt}}, path::Path,
};

const PENDING: &[u8] = b"INITIALIZING";
const FLOOR: &[u8] = b"CONFIRMED";

fn bad(message: &'static str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message) }
fn unsupported(message: &'static str) -> io::Error { io::Error::new(io::ErrorKind::Unsupported, message) }

// No member of a Stamp is a persistent identity. The held FD prevents inode
// reuse within the observation; directory and data locks require cooperation.
#[derive(Eq, PartialEq)]
struct Stamp {
    entity: Entity, mount: u64, kind: std::fs::FileType, uid: u32, mode: u32, links: u64, len: u64,
    modified: (i64, i64), changed: (i64, i64),
}

impl Stamp {
    fn on(file: &File) -> io::Result<Self> {
        let m = file.metadata()?;
        let mount = Mount::on(file)?.ok_or_else(|| unsupported("mount identity is unavailable"))?;
        Ok(Self { entity: Entity::of(&m)?, mount: mount.id, kind:m.file_type(), uid:m.uid(), mode:m.mode(),
            links:m.nlink(), len:m.len(), modified:(m.mtime(),m.mtime_nsec()), changed:(m.ctime(),m.ctime_nsec()) })
    }

    fn root(file: &File, uid: u32) -> io::Result<Self> {
        let s = Self::on(file)?;
        if !s.kind.is_dir() || s.uid != uid || s.mode & 0o022 != 0 || s.links == 0 {
            return Err(bad("SYSTEM is not the protected directory of the selected storage account"));
        }
        Ok(s)
    }
}

fn pin(dir: &File, name: &[u8]) -> io::Result<File> {
    Ok(File::from(openat(dir, OsStr::from_bytes(name),
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())?))
}

struct Record { name: &'static [u8], fd: File, original: Stamp }
impl Record {
    fn open(dir: &File, parent: &Stamp, name: &'static [u8]) -> io::Result<Self> {
        let fixed = pin(dir, name)?;
        let s = Stamp::on(&fixed)?;
        if !s.kind.is_file() || s.uid != parent.uid || s.mode & 0o7777 != 0o600
            || s.links != 1 || s.mount != parent.mount || s.entity.dev != parent.entity.dev {
            return Err(bad("control name does not designate a private, singly linked regular file on SYSTEM"));
        }
        let size_ok = match name {
            PENDING => (1..=MAX_GENESIS as u64).contains(&s.len),
            FLOOR => s.len == FLOOR_LEN as u64,
            ROOT_SLOT => s.len == CLAIM_LEN as u64,
            _ => false,
        };
        if !size_ok { return Err(bad("control length outside the initial-registration format")); }
        // This is a fixed control name on a separately selected storage origin,
        // not arbitrary data supplied to compare-files. A path re-open could hit a device.
        let fd = reopen_admitted_reader(&fixed, s.entity, ReaderBoundary::SelectedControlMount(s.mount))?;
        if Stamp::on(&fd)? != s { return Err(bad("control changed while opening its original descriptor")); }
        Ok(Self { name, fd, original:s })
    }

    fn bytes(&self) -> io::Result<Vec<u8>> {
        if Stamp::on(&self.fd)? != self.original { return Err(bad("control changed before read")); }
        let mut out = vec![0; self.original.len as usize]; // bounded at the O_PATH pin, before opening for data
        self.fd.read_exact_at(&mut out, 0)?;
        loop {
            match self.fd.read_at(&mut [0], self.original.len) {
                Ok(0) => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
                Ok(_) => return Err(bad("unexpected bytes after the control record")),
            }
        }
        if Stamp::on(&self.fd)? != self.original { return Err(bad("control changed during read")); }
        Ok(out)
    }

    fn require_name(&self, dir: &File) -> io::Result<()> {
        if Stamp::on(&pin(dir, self.name)?)? != self.original || Stamp::on(&self.fd)? != self.original {
            return Err(bad("control name or held file no longer matches its original"));
        }
        Ok(())
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
struct Names { floor: bool, marker: bool }
impl Names {
    fn read(directory: File) -> io::Result<Self> {
        // This fresh independent description starts at offset zero. RawDir
        // propagates directory read errors rather than calling them a clean EOF.
        let mut buf = [MaybeUninit::<u8>::uninit(); 4096];
        let mut entries = RawDir::new(directory, &mut buf);
        let (mut dot, mut parent, mut pending, mut floor, mut marker) = (false, false, false, false, false);
        while let Some(entry) = entries.next() {
            match entry?.file_name().to_bytes() {
                b"." if !dot => dot = true,
                b".." if !parent => parent = true,
                PENDING if !pending => pending = true,
                FLOOR if !floor => floor = true,
                ROOT_SLOT if !marker => marker = true,
                _ => return Err(bad("published, foreign or repeated entry in initial-registration SYSTEM")),
            }
        }
        if !pending || (marker && !floor) {
            return Err(bad("no unique pending name, or SYSTEM marker without its confirmation file"));
        }
        Ok(Self {floor,marker})
    }
}

/// Only the local SYSTEM stage. No observation of the recycle or data roots is implied.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub(crate) enum SystemPhase { NoFloor, ZeroFloor, ZeroFloorAndSystemMarker }

/// Keeps the observed registration's nonblocking EX on the very same read-only OFD.
/// No FD/writer is exposed. The short SYSTEM lock is taken afresh for each sample.
#[must_use]
pub(crate) struct HeldPending {
    path: Pinned, system: File, uid:u32, register: Record, floor: Option<Record>, marker: Option<Record>,
}
impl HeldPending {
    /// host_origin must come from the trusted deployment's independently held
    /// original directory, NOT by reopening a pathname from GENESIS. Equality
    /// below is real-time attribution, not authentication of whoever supplied it.
    pub(crate) fn open(host_origin: &File, selected: &Path, uid: u32, expected_body: &[u8])
        -> io::Result<(Self, SystemPhase)> {
        if uid == 0 || uid == u32::MAX || expected_body.is_empty() || expected_body.len() > MAX_GENESIS-32 {
            return Err(bad("expected the original request and protected non-root storage owner"));
        }
        let origin = Stamp::root(host_origin, uid)?;
        let path = Pinned::open(selected, Kind::StorageRoot)?;
        // Only O_PATH + metadata on the spelling until its actual entity, mount
        // and protection match the independently supplied host origin.
        if Stamp::root(&path.fd, uid)? != origin {
            return Err(bad("selected SYSTEM differs from supplied host origin"));
        }
        let system = path.storage_directory()?;
        let here = Stamp::root(&system, uid)?;
        if here != origin { return Err(bad("original SYSTEM changed during its directory open")); }

        // Lookup may fold case or normalize names. Do not open/lock a
        // lookalike whose exact raw directory entry was never present. This
        // precheck is not a cut: enumerate again under the actual two locks.
        Names::read(path.storage_directory()?)?;
        let register = Record::open(&system, &here, PENDING)?;
        flock(&register.fd, FlockOperation::NonBlockingLockExclusive)?;
        let cut = path.storage_directory()?;
        flock(&cut, FlockOperation::NonBlockingLockShared)?;
        let before = Stamp::root(&cut, uid)?;
        if before != Stamp::root(&system, uid)? { return Err(bad("SYSTEM changed at lock acquisition")); }
        let names = Names::read(path.storage_directory()?)?;
        let floor = names.floor.then(|| Record::open(&cut, &before, FLOOR)).transpose()?;
        let marker = names.marker.then(|| Record::open(&cut, &before, ROOT_SLOT)).transpose()?;
        let held = Self {path,system,uid,register,floor,marker};
        let stage = held.sample(&cut, &before, expected_body)?;
        Ok((held, stage))
    }

    pub(crate) fn recheck(&self, expected_body:&[u8]) -> io::Result<SystemPhase> {
        let cut = self.path.storage_directory()?;
        flock(&cut, FlockOperation::NonBlockingLockShared)?;
        let before = Stamp::root(&cut,self.uid)?;
        self.sample(&cut,&before,expected_body)
    }

    fn sample(&self, cut:&File, before:&Stamp, expected_body:&[u8]) -> io::Result<SystemPhase> {
        if Stamp::root(&self.system,self.uid)? != *before { return Err(bad("SYSTEM changed before observation")); }
        self.register.require_name(cut)?;
        let bytes = self.register.bytes()?;
        let initial = Initial::decode(&bytes).map_err(|e| match e {
            Invalid::TailNotCovered => unsupported("a register with successor bytes is not an empty pending operation"),
            _ => bad("not one intact portable initial registration"),
        })?;
        initial.check_request(expected_body).map_err(|_| bad("not the independently selected original request"))?;
        let (limit, expected_marker) = initial.system_row(self.path.path().as_os_str().as_bytes(),self.uid)
            .map_err(|_| bad("original request does not select this SYSTEM and storage owner"))?;
        if helper_linux_evidence::portable_component_limit(cut)? != limit {
            return Err(bad("SYSTEM component limit differs from its initial row"));
        }
        if let Some(f) = &self.floor {
            initial.check_zero_floor(&f.bytes()?).map_err(|_| bad("not this registration's exact zero confirmation"))?;
        }
        if let Some(m) = &self.marker {
            expected_marker.check_row(&m.bytes()?).map_err(|_| bad("SYSTEM marker does not match the original claim"))?;
        }
        let named = self.path.storage_directory()?;
        let expected = Names { floor: self.floor.is_some(), marker: self.marker.is_some() };
        if Names::read(named)? != expected { return Err(bad("SYSTEM controls changed")); }
        for r in [Some(&self.register),self.floor.as_ref(),self.marker.as_ref()].into_iter().flatten() { r.require_name(cut)?; }
        if Stamp::root(cut,self.uid)? != *before || Stamp::root(&self.path.storage_directory()?,self.uid)? != *before {
            return Err(bad("SYSTEM changed while checking the pending registration"));
        }
        match (expected.floor,expected.marker) {
            (false,false) => Ok(SystemPhase::NoFloor),
            (true,false) => Ok(SystemPhase::ZeroFloor),
            (true,true) => Ok(SystemPhase::ZeroFloorAndSystemMarker),
            (false,true) => Err(bad("SYSTEM marker without a confirmation file")),
        }
    }
}

#[cfg(test)]
#[path = "../../tests/portable/pending_system.rs"]
mod tests;
