//! One portable, self-contained, terminal range table for a specific formal
//! node. The digest is over these exact bytes: not a flat SHA of the file
//! contents and not an indication that a SYSTEM has issued this node.
use super::{Build, Index, Leaf, Material, MAX_ROWS};
use crate::{FileId, NodeId, OperationId, PoolId, VersionId, fs::invalid, hash};
use crate::ordinary::wire::In;
use std::{fs::File, io, os::unix::fs::FileExt};

const MAGIC: &[u8] = b"ZXS-PORTABLE-1/RANGE-INDEX\0";
const FIXED: usize = MAGIC.len() + 4 * 16 + 8 + 8 + 4 + 4;
const MATERIAL: usize = 16 + 4 + 8;
const ROW: usize = 8 + 4 + 8;
const MAX: usize = FIXED + MAX_ROWS * (MATERIAL + ROW);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ordinary) struct VersionKey { pub pool: PoolId, pub file: FileId, pub node: NodeId, pub version: VersionId, pub representation: u64 }
impl VersionKey {
    fn require(self) -> io::Result<Self> {
        if [self.pool.0, self.file.0, self.node.0, self.version.0].contains(&[0;16]) {
            return Err(invalid("missing identity in portable version index"));
        }
        Ok(self)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ordinary) struct IndexDigest([u8;32]);

/// Descriptor of a physical index representation. A SYSTEM decision must
/// bind it; constructing it in memory cannot grant access or publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ordinary) struct Descriptor {
    pub key: VersionKey, pub logical_bytes: u64, pub index_bytes: u64, pub digest: IndexDigest,
}

fn size(materials: usize, rows: usize) -> io::Result<usize> {
    if rows > MAX_ROWS || materials > rows {
        return Err(invalid("portable index exceeds its actual row/material budget"));
    }
    Ok(FIXED + materials * MATERIAL + rows * ROW) // bounded above by MAX
}

/// Return bytes only. No object is named, persisted or published here.
pub(in crate::ordinary) fn encode(key: VersionKey, index: &Index) -> io::Result<(Descriptor,Vec<u8>)> {
    key.require()?;
    if key.file != index.file() { return Err(invalid("cannot encode another FILE's index under this node")); }
    let count = size(index.materials.len(), index.rows.len())?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(count).map_err(|_| invalid("cannot reserve portable version index"))?;
    bytes.extend_from_slice(MAGIC);
    for id in [key.pool.0, key.file.0, key.node.0, key.version.0] { bytes.extend_from_slice(&id); }
    bytes.extend_from_slice(&key.representation.to_be_bytes());
    bytes.extend_from_slice(&index.len().to_be_bytes());
    for n in [index.materials.len(), index.rows.len()] { bytes.extend_from_slice(&(n as u32).to_be_bytes()); }
    for m in &index.materials {
        bytes.extend_from_slice(&m.issuer.0); bytes.extend_from_slice(&m.slot.to_be_bytes()); bytes.extend_from_slice(&m.size.to_be_bytes());
    }
    for r in &index.rows {
        bytes.extend_from_slice(&r.end.to_be_bytes());
        bytes.extend_from_slice(&r.terminal.map_or(0, |n| n+1).to_be_bytes());
        bytes.extend_from_slice(&r.offset.to_be_bytes());
    }
    let reference = Descriptor::declared(key,index.len(),bytes.len() as u64,hash::digest(&bytes)?)?;
    Ok((reference,bytes))
}

impl Descriptor {
    /// Typed import/export for the original reference supplied separately from
    /// the table. This is structure checking, never proof that it was issued.
    pub(in crate::ordinary) fn declared(key:VersionKey,logical_bytes:u64,index_bytes:u64,index_sha:[u8;32]) -> io::Result<Self> {
        key.require()?;
        if logical_bytes>i64::MAX as u64 || !(FIXED as u64..=MAX as u64).contains(&index_bytes) {
            return Err(invalid("invalid external portable index descriptor"));
        }
        Ok(Self {key,logical_bytes,index_bytes,digest:IndexDigest(index_sha)})
    }
    pub(in crate::ordinary) fn index_sha(self)->[u8;32] { self.digest.0 }

    /// The expected descriptor must come from outside the bytes being read.
    /// The index contains no mutable path, previous-node pointer, or trailing
    /// checksum that could be presented as the original published descriptor.
    pub(in crate::ordinary) fn decode(self, bytes: &[u8]) -> io::Result<Index> {
        if bytes.len() as u64 != self.index_bytes { return Err(invalid("not the original portable index length")); }
        let header=bytes.get(..FIXED).ok_or_else(||invalid("short portable index header"))?;
        let (nm,nr)=self.counts(header)?;
        self.decode_body(bytes,nm,nr)
    }

    // Validate the exact shape and bound before reserving for attacker-
    // controlled counts. The expected descriptor itself is never a grant.
    fn counts(self, header: &[u8]) -> io::Result<(usize,usize)> {
        let key=self.key.require()?;
        let mut r=In(header);
        if self.logical_bytes > i64::MAX as u64 || r.take(MAGIC.len())? != MAGIC || r.id()? != key.pool.0
            || r.id()? != key.file.0 || r.id()? != key.node.0 || r.id()? != key.version.0
            || r.u64()? != key.representation || r.u64()? != self.logical_bytes {
            return Err(invalid("portable index is not this exact FILE/node/version/representation"));
        }
        let nm=r.u32()? as usize;let nr=r.u32()? as usize;r.end()?;
        if size(nm,nr)? as u64 != self.index_bytes || (self.logical_bytes==0) != (nr==0) {
            return Err(invalid("portable index table size disagrees with the expected descriptor"));
        }
        Ok((nm,nr))
    }

    // Always hash precisely the input buffer that we decode. In particular,
    // no seek/re-read of a now-mutable header after verifying its hash.
    fn decode_body(self, bytes: &[u8], nm:usize, nr:usize) -> io::Result<Index> {
        if IndexDigest(hash::digest(bytes)?) != self.digest { return Err(invalid("wrong original portable index digest")); }
        let key=self.key;
        let mut r=In(&bytes[FIXED..]);
        let mut materials = Vec::new();
        materials.try_reserve_exact(nm).map_err(|_| invalid("cannot reserve portable index materials"))?;
        for _ in 0..nm {
            let m = Material::declared(key.file, OperationId(r.id()?), r.u32()?, r.u64()?)?;
            if m.size == 0 { return Err(invalid("unused empty material in portable descriptor table")); }
            materials.push(m);
        }
        let mut builder = Build::new(key.file)?;
        let mut start = 0;
        for i in 0..nr {
            let end = r.u64()?; let term = r.u32()?; let offset = r.u64()?;
            if end <= start || end > self.logical_bytes { return Err(invalid("invalid portable logical interval")); }
            let leaf = match term.checked_sub(1) {
                None => Leaf::Zero,
                Some(n) => Leaf::Data(*materials.get(n as usize).ok_or_else(|| invalid("unknown direct material ordinal"))?),
            };
            builder.add(leaf, offset, end-start)?;
            // Builder canonicalizes. A valid on-disk table is *already*
            // canonical, including first-use numbering and truly unique IDs.
            if builder.index.rows.len() != i+1 || builder.index.rows[i].terminal.map_or(0, |n|n+1) != term {
                return Err(invalid("noncanonical adjacent ranges or portable material ordering"));
            }
            start = end;
        }
        r.end()?;
        if builder.index.materials.len() != nm { return Err(invalid("unreachable or duplicate portable material")); }
        builder.finish(self.logical_bytes)
    }

    /// Read the bounded table from an actually open ordinary regular file,
    /// without disturbing its seek offset. No parent table or BODY is opened
    /// to rehydrate its index. Custody, naming and payload verification are
    /// distinct responsibilities of callers with actual original authority.
    pub(in crate::ordinary) fn read(self, file: &File) -> io::Result<Index> {
        let n = usize::try_from(self.index_bytes).ok().filter(|n| (FIXED..=MAX).contains(n))
            .ok_or_else(|| invalid("unusable original portable table length"))?;
        let check = || -> io::Result<()> {
            let m=file.metadata()?;
            if !m.is_file() || m.len()!=self.index_bytes { return Err(invalid("not the exact-size regular portable index")); }
            Ok(())
        };
        check()?;let mut header=[0;FIXED];file.read_exact_at(&mut header,0)?;
        let (nm,nr)=self.counts(&header)?;
        let mut bytes=Vec::new();bytes.try_reserve_exact(n).map_err(|_|invalid("cannot read portable index within memory budget"))?;
        bytes.extend_from_slice(&header);bytes.resize(n,0);file.read_exact_at(&mut bytes[FIXED..],FIXED as u64)?;
        let view=self.decode_body(&bytes,nm,nr)?;check()?;Ok(view)
    }
}

#[cfg(test)]
#[path="../../../tests/unit/flat_stored.rs"]
mod tests;
