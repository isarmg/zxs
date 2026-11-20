//! Canonical terminal range tables: a formal representation and a work
//! revision have different domains but exactly one row/ordinal codec.
//! Coordinates and digests describe bytes; they do not confer authority.
use super::{Build, Index, Leaf, Material, MAX_ROWS};
use crate::{FileId, NodeId, OperationId, PoolId, VersionId, fs::{HeldBytes,invalid}, hash};
use crate::ordinary::wire::In;
use std::{fs::File, io, os::unix::fs::FileExt};

const MAGIC: &[u8] = b"ZXS-PORTABLE-1/RANGE-INDEX\0";
const WORK: &[u8] = b"ZXS-PORTABLE-1/DRAFT-INDEX\0";
const _: () = assert!(MAGIC.len()==WORK.len());
const FIXED: usize = MAGIC.len() + 4 * 16 + 8 + 8 + 4 + 4;
const MATERIAL: usize = 16 + 4 + 8;
const ROW: usize = 8 + 4 + 8;
const MAX: usize = FIXED + MAX_ROWS * (MATERIAL + ROW);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ordinary) struct VersionKey { pub pool: PoolId, pub file: FileId, pub node: NodeId, pub version: VersionId, pub representation: u64 }
impl VersionKey{
    fn scope(self)->Scope{Scope{magic:MAGIC,pool:self.pool,file:self.file,ids:[self.node.0,self.version.0],number:self.representation}}
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) struct WorkKey{pub(super) pool:PoolId,pub(super) file:FileId,pub(super) round:OperationId,pub(super) save:OperationId,pub(super) revision:u64}
impl WorkKey{
    fn scope(self)->io::Result<Scope>{
        if self.revision==0||self.revision>i64::MAX as u64||(self.revision==1)!=(self.round==self.save){
            return Err(invalid("invalid selected working revision identity"));
        }
        Ok(Scope{magic:WORK,pool:self.pool,file:self.file,ids:[self.round.0,self.save.0],number:self.revision})
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ordinary) struct IndexDigest([u8;32]);

/// Only a SYSTEM decision can bind the descriptor to a published node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ordinary) struct Descriptor {
    pub key: VersionKey, pub logical_bytes: u64, pub index_bytes: u64, pub digest: IndexDigest,
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) struct WorkDescriptor{key:WorkKey,table:Table}
impl WorkDescriptor{
    pub(super) fn declared(key:WorkKey,logical_bytes:u64,index_bytes:u64,sha:[u8;32])->io::Result<Self>{
        let table=Table::declared(key.scope()?,logical_bytes,index_bytes,sha)?;
        Ok(Self{key,table})
    }
    pub(super) fn key(self)->WorkKey{self.key}
    pub(super) fn logical_bytes(self)->u64{self.table.logical_bytes}
    pub(super) fn index_bytes(self)->u64{self.table.index_bytes}
    pub(super) fn index_sha(self)->[u8;32]{self.table.digest.0}
    /// The storage caller must already have selected this exact revision,
    /// the true checksum pair, and the confirmed physical boundary.
    /// Never extend the selected window to the actual underlying EOF.
    pub(super) fn read_selected(self,checks:&HeldBytes,at:u64,selected_end:u64)->io::Result<Index>{
        if selected_end>checks.stamp().len||selected_end>i64::MAX as u64||
           at.checked_add(self.table.index_bytes).is_none_or(|end|end>selected_end){
            return Err(invalid("work index lies outside the selected checksum window"));
        }
        checks.unchanged()?;
        let decoded=self.table.read_at(checks.file(),at)?;
        checks.unchanged()?;Ok(decoded)
    }
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct Scope{magic:&'static[u8],pool:PoolId,file:FileId,ids:[[u8;16];2],number:u64}
impl Scope{
    fn require(self)->io::Result<Self>{
        if [self.pool.0,self.file.0,self.ids[0],self.ids[1]].contains(&[0;16]){
            return Err(invalid("missing identity in portable direct index"));
        }
        Ok(self)
    }
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct Table{scope:Scope,logical_bytes:u64,index_bytes:u64,digest:IndexDigest}
fn size(materials: usize, rows: usize) -> io::Result<usize> {
    if rows > MAX_ROWS || materials > rows {
        return Err(invalid("portable index exceeds its actual row/material budget"));
    }
    Ok(FIXED + materials * MATERIAL + rows * ROW) // bounded above by MAX
}
fn serialize(scope:Scope,index:&Index)->io::Result<(Table,Vec<u8>)>{
    scope.require()?;
    if scope.file!=index.file()||scope.magic==WORK&&index.materials.iter().any(|m|m.slot!=0){
        return Err(invalid("index describes a different FILE or an unissued working slot"));
    }
    let count = size(index.materials.len(), index.rows.len())?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(count).map_err(|_| invalid("cannot reserve portable version index"))?;
    bytes.extend_from_slice(scope.magic);
    for id in [scope.pool.0,scope.file.0,scope.ids[0],scope.ids[1]] { bytes.extend_from_slice(&id); }
    bytes.extend_from_slice(&scope.number.to_be_bytes());
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
    let table=Table::declared(scope,index.len(),bytes.len() as u64,hash::digest(&bytes)?)?;
    Ok((table,bytes))
}
pub(in crate::ordinary) fn encode(key:VersionKey,index:&Index)->io::Result<(Descriptor,Vec<u8>)>{
    let (t,b)=serialize(key.scope(),index)?;
    Ok((Descriptor{key,logical_bytes:t.logical_bytes,index_bytes:t.index_bytes,digest:t.digest},b))
}
pub(super) fn encode_work(key:WorkKey,index:&Index)->io::Result<(WorkDescriptor,Vec<u8>)>{
    let (table,b)=serialize(key.scope()?,index)?;
    Ok((WorkDescriptor{key,table},b))
}

impl Descriptor {
    /// Structure check; whether the external descriptor was published
    /// remains an independent decision of the original SYSTEM.
    pub(in crate::ordinary) fn declared(key:VersionKey,logical_bytes:u64,index_bytes:u64,index_sha:[u8;32])->io::Result<Self>{
        let t=Table::declared(key.scope(),logical_bytes,index_bytes,index_sha)?;
        Ok(Self{key,logical_bytes,index_bytes,digest:t.digest})
    }
    fn table(self)->Table{Table{scope:self.key.scope(),logical_bytes:self.logical_bytes,index_bytes:self.index_bytes,digest:self.digest}}
    pub(in crate::ordinary) fn index_sha(self)->[u8;32]{self.digest.0}
    pub(in crate::ordinary) fn decode(self,bytes:&[u8])->io::Result<Index>{
        if bytes.len() as u64!=self.index_bytes{return Err(invalid("not the original portable index length"));}
        let t=self.table();let(nm,nr)=t.counts(bytes.get(..FIXED).ok_or_else(||invalid("short portable index header"))?)?;
        t.decode_body(bytes,nm,nr)
    }
    /// The formal version index is still a complete, exact-size real file.
    pub(in crate::ordinary) fn read(self,file:&File)->io::Result<Index>{
        let check=||->io::Result<()>{
            let m=file.metadata()?;
            if !m.is_file()||m.len()!=self.index_bytes{return Err(invalid("not the exact-size regular portable index"));}
            Ok(())
        };
        check()?;let view=self.table().read_at(file,0)?;check()?;Ok(view)
    }
}
impl Table{
    fn declared(scope:Scope,logical_bytes:u64,index_bytes:u64,sha:[u8;32])->io::Result<Self>{
        scope.require()?;
        if logical_bytes>i64::MAX as u64||!(FIXED as u64..=MAX as u64).contains(&index_bytes){
            return Err(invalid("invalid external portable index descriptor"));
        }
        Ok(Self{scope,logical_bytes,index_bytes,digest:IndexDigest(sha)})
    }
    // Validate the exact shape and bound before reserving for untrusted
    // counts. A caller-chosen digest still cannot legitimize malformed rows.
    fn counts(self, header: &[u8]) -> io::Result<(usize,usize)> {
        let scope=self.scope.require()?;
        let mut r=In(header);
        if self.logical_bytes>i64::MAX as u64||r.take(scope.magic.len())?!=scope.magic||r.id()?!=scope.pool.0||
           r.id()?!=scope.file.0||r.id()?!=scope.ids[0]||r.id()?!=scope.ids[1]||
           r.u64()?!=scope.number||r.u64()?!=self.logical_bytes{
            return Err(invalid("portable index belongs to a different selected identity or domain"));
        }
        let nm=r.u32()? as usize;let nr=r.u32()? as usize;r.end()?;
        if size(nm,nr)? as u64 != self.index_bytes || (self.logical_bytes==0) != (nr==0) {
            return Err(invalid("portable index table size disagrees with the expected descriptor"));
        }
        Ok((nm,nr))
    }
    // Hash the exact buffer decoded below, not a header re-read from a new
    // position or another physical view after digest verification.
    fn decode_body(self,bytes:&[u8],nm:usize,nr:usize)->io::Result<Index>{
        if IndexDigest(hash::digest(bytes)?) != self.digest { return Err(invalid("wrong original portable index digest")); }
        let scope=self.scope;
        let mut r=In(&bytes[FIXED..]);
        let mut materials = Vec::new();
        materials.try_reserve_exact(nm).map_err(|_| invalid("cannot reserve portable index materials"))?;
        for _ in 0..nm {
            let m = Material::declared(scope.file, OperationId(r.id()?), r.u32()?, r.u64()?)?;
            if m.size==0||scope.magic==WORK&&m.slot!=0{return Err(invalid("unused or invalid slot in portable descriptor table"));}
            materials.push(m);
        }
        let mut builder = Build::new(scope.file)?;
        let mut start = 0;
        for i in 0..nr {
            let end = r.u64()?; let term = r.u32()?; let offset = r.u64()?;
            if end <= start || end > self.logical_bytes { return Err(invalid("invalid portable logical interval")); }
            let leaf = match term.checked_sub(1) {
                None => Leaf::Zero,
                Some(n) => Leaf::Data(*materials.get(n as usize).ok_or_else(|| invalid("unknown direct material ordinal"))?),
            };
            builder.add(leaf, offset, end-start)?;
            if builder.index.rows.len() != i+1 || builder.index.rows[i].terminal.map_or(0, |n| n+1) != term {
                return Err(invalid("noncanonical adjacent ranges or portable material ordering"));
            }
            start = end;
        }
        r.end()?;
        if builder.index.materials.len() != nm { return Err(invalid("unreachable or duplicate portable material")); }
        builder.finish(self.logical_bytes)
    }
    fn read_at(self,file:&File,at:u64)->io::Result<Index>{
        let n=usize::try_from(self.index_bytes).ok().filter(|n|(FIXED..=MAX).contains(n))
            .ok_or_else(||invalid("unusable original portable table length"))?;
        if at.checked_add(n as u64).is_none_or(|v|v>i64::MAX as u64){return Err(invalid("portable index offset overflow"));}
        let mut header=[0;FIXED];file.read_exact_at(&mut header,at)?;
        let(nm,nr)=self.counts(&header)?;
        let mut bytes=Vec::new();bytes.try_reserve_exact(n).map_err(|_|invalid("cannot read portable index within memory budget"))?;
        bytes.extend_from_slice(&header);bytes.resize(n,0);file.read_exact_at(&mut bytes[FIXED..],at+FIXED as u64)?;
        self.decode_body(&bytes,nm,nr)
    }
}

#[cfg(test)]
#[path="../../../tests/unit/flat_stored.rs"]
mod tests;

#[cfg(test)]
#[path="../../../tests/unit/flat_work_index.rs"]
mod working_tests;
