//! A fresh SYSTEM image has one genesis, not a journal or an account database.
//! Native paths, kernel handles and IDs are bytes, not display strings.
use crate::path_rules::{Kind, MAX_PATH_BYTES};
use sha2::{Digest, Sha256};

mod encoding;
pub(super) use encoding::{InitialBytes, RootInput};

const GENESIS: &[u8] = b"ZXS/FILE-POOL/MAINT-1\0";
const VOLUME: &[u8] = b"ZXS/VOLUME/MAINT-1\0";
const MAX_IMAGE: usize = 64 * (MAX_PATH_BYTES + 64 + 148) + 256;
type Id = [u8; 16];

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Invalid {
    Format, Length, Identity, Path, Role, Digest, Label, Frontier,
}

/// One original object's filesystem, inode and complete filesystem-scoped birth.
/// The kernel handle is only for comparison. It must never be used to open an object.
#[derive(Debug, PartialEq, Eq)]
struct Original<'a> {
    device: u64,
    inode: u64,
    birth: &'a [u8], // UUID[16], big-endian kernel handle type[4], opaque handle[1..128]
}
impl Original<'_> {
    fn filesystem(&self) -> &[u8] { &self.birth[..16] }
    fn aliases(&self, other: &Self) -> bool {
        (self.device == other.device && self.inode == other.inode) || self.birth == other.birth
    }
}
#[derive(Debug)]
struct Volume<'a> {
    tag: u8,
    id: Id,
    object: Original<'a>,
    name_max: u8,
    path: &'a [u8],
}

/// Evidence about supplied bytes only. A caller still needs the actual original
/// registration file and every original root, current owner/mounts and live locks.
#[derive(Debug)]
pub(super) struct FreshGenesis<'a> {
    pool: Id,
    operation: Id,
    storage_uid: u32,
    registration: Original<'a>,
    volumes: Vec<Volume<'a>>,
    digest: [u8; 32],
}

// Inputs to this comparison have already passed exact component validation.
fn same_or_below(path: &[u8], parent: &[u8]) -> bool {
    path.strip_prefix(parent).is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(b"/"))
}

struct Cursor<'a>(&'a [u8]);
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Invalid> {
        let (value, rest) = self.0.split_at_checked(n).ok_or(Invalid::Length)?;
        self.0 = rest;
        Ok(value)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], Invalid> {
        self.take(N)?.try_into().map_err(|_| Invalid::Length)
    }
    fn byte(&mut self) -> Result<u8, Invalid> { Ok(self.array::<1>()?[0]) }
    fn id(&mut self) -> Result<Id, Invalid> {
        let id = self.array()?;
        if id == [0; 16] { return Err(Invalid::Identity); }
        Ok(id)
    }
    fn object(&mut self) -> Result<Original<'a>, Invalid> {
        let device = u64::from_be_bytes(self.array()?);
        let inode = u64::from_be_bytes(self.array()?);
        let n = usize::from(self.byte()?);
        if inode == 0 || !(21..=148).contains(&n) { return Err(Invalid::Identity); }
        let birth = self.take(n)?;
        if birth[..16] == [0; 16] { return Err(Invalid::Identity); }
        Ok(Original { device, inode, birth })
    }
    fn path(&mut self) -> Result<&'a [u8], Invalid> {
        let n = usize::from(u16::from_be_bytes(self.array()?));
        if !(2..=MAX_PATH_BYTES).contains(&n) { return Err(Invalid::Path); }
        let path = self.take(n)?;
        Kind::StorageRoot.check_path(path).map_err(|_| Invalid::Path)?;
        Ok(path)
    }
}

impl<'a> FreshGenesis<'a> {
    /// Only an intact initial image with no trailing business bytes. Never open
    /// a saved path on the strength of its checksum or a matching stored UID.
    pub(super) fn decode(image: &'a [u8]) -> Result<Self, Invalid> {
        if image.len() > MAX_IMAGE { return Err(Invalid::Length); }
        let (body, stored) = image.split_last_chunk::<32>().ok_or(Invalid::Length)?;
        let mut c = Cursor(body.strip_prefix(GENESIS).ok_or(Invalid::Format)?);
        let digest: [u8; 32] = Sha256::digest(body).into();
        if stored != &digest { return Err(Invalid::Digest); }
        let pool = c.id()?;
        let operation = c.id()?;
        let storage_uid = u32::from_be_bytes(c.array()?);
        let registration = c.object()?;
        let count = usize::from(u16::from_be_bytes(c.array()?));
        if !(3..=64).contains(&count) { return Err(Invalid::Role); }
        let mut volumes: Vec<Volume<'a>> = Vec::with_capacity(count);
        for i in 0..count {
            let tag = c.byte()?;
            let role = match i { 0 => 0, 1 => 1, _ => 2 };
            if tag != role { return Err(Invalid::Role); }
            let id = c.id()?;
            let object = c.object()?;
            let name_max = c.byte()?;
            if name_max == 0 { return Err(Invalid::Path); }
            let path = c.path()?;
            if registration.aliases(&object) || volumes.iter().any(|v| v.id == id || v.object.aliases(&object)) {
                return Err(Invalid::Identity);
            }
            if volumes.iter().any(|v| same_or_below(v.path, path) || same_or_below(path, v.path))
                || (i > 2 && volumes[i - 1].path >= path) {
                return Err(Invalid::Path);
            }
            volumes.push(Volume { tag, id, object, name_max, path });
        }
        if !c.0.is_empty() { return Err(Invalid::Length); }
        let system = &volumes[0].object;
        let recycle = &volumes[1].object;
        if registration.device != system.device || registration.filesystem() != system.filesystem()
            || volumes.iter().filter(|v| v.tag != 1).any(|v| {
                v.object.device == recycle.device || v.object.filesystem() == recycle.filesystem()
            }) {
            return Err(Invalid::Identity);
        }
        Ok(Self { pool, operation, storage_uid, registration, volumes, digest })
    }

    /// Compares only these two observed originals, never the other volumes, a
    /// pathname, label, lock or operator authority. Mount IDs are not stored.
    pub(super) fn matches_origin_pair(&self, uid: u32, registration: (u64, u64, &[u8]),
                                      system: (u64, u64, &[u8])) -> bool {
        let r = &self.registration;
        let s = &self.volumes[0].object;
        self.storage_uid == uid && registration == (r.device, r.inode, r.birth)
            && system == (s.device, s.inode, s.birth)
    }

    pub(super) fn has_root_count(&self, uid: u32, count: usize) -> bool {
        self.storage_uid == uid && self.volumes.len() == count
    }

    /// Both the wire decoder and caller use S, R, then sorted data rows.
    pub(super) fn matches_root_origin(&self, index: usize, path: &[u8], name_max: u8, observed: (u64, u64, &[u8])) -> bool {
        self.volumes.get(index).is_some_and(|row| row.path == path && row.name_max == name_max
            && observed == (row.object.device, row.object.inode, row.object.birth))
    }

    /// All labels must appear once, in original registration order. Only the
    /// SYSTEM volume binds the genesis digest; data/recycle labels do not.
    pub(super) fn check_labels(&self, labels: &[&[u8]]) -> Result<(), Invalid> {
        if labels.len() != self.volumes.len() { return Err(Invalid::Label); }
        for (v, &label) in self.volumes.iter().zip(labels) {
            let tail = label.strip_prefix(VOLUME).and_then(|b| b.strip_prefix(&[v.tag]))
                .and_then(|b| b.strip_prefix(&self.pool[..]))
                .and_then(|b| b.strip_prefix(&v.id[..]))
                .and_then(|b| b.strip_prefix(&self.operation[..]));
            let expected: &[u8] = if v.tag == 0 { &self.digest } else { &[] };
            if tail != Some(expected) { return Err(Invalid::Label); }
        }
        Ok(())
    }

    /// None is not a zero frontier. A fresh inode acknowledges no business
    /// sequence or frame bytes yet; its 32-byte head is still mandatory.
    pub(super) fn check_initial_frontier(&self, observed: Option<&[u8]>) -> Result<(), Invalid> {
        if observed.and_then(|bytes| bytes.strip_prefix(&[0; 16])) == Some(&self.digest[..]) {
            Ok(())
        } else { Err(Invalid::Frontier) }
    }
}

#[cfg(test)]
#[path = "../../tests/maintenance/genesis.rs"]
mod tests;
