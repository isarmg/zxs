//! The mkdir operand occupies the existing single SYSTEM allocation name.
//! Bytes identify the issuing native inode. Its bounded stage attribute binds
//! at most one freshly created child, never an existing directory by spelling.
//! Decoding is not admission, nor does this module publish an allocation.
use super::types::{self,Created,Origin};
use crate::{PoolId,fs::{self,HeldDir,invalid},ordinary::{allocation::{self,Stage},wire::{In,NativeObject}}};
use std::io;

pub(in crate::ordinary) const MAGIC:&[u8]=b"ZXS/ORIGINAL-ORDINARY-MKDIR/1\0";
const MAX:usize=MAGIC.len()+17+fs::MAX_BIRTH+16+types::MAX_ORIGIN+8+32+32+32;
const _:()=assert!(MAX<=allocation::MAX);
pub(in crate::ordinary) struct Operand{
    pool:PoolId,pub origin:Origin,pub seq:u64,pub previous:[u8;32],pub request:[u8;32],
    pub digest:[u8;32],pub stamp:fs::OriginalStamp,pub bytes:Vec<u8>,pub stage:Option<Stage>,
}
impl Operand{
    /// Prepare bounded payload bytes only. No register/descriptor is handed
    /// out; an eventual issuer must check the live writer, namespace and capacity.
    pub(super) fn image(pool:PoolId,issuer:&NativeObject,origin:&Origin,seq:u64,previous:[u8;32])->io::Result<(Vec<u8>,[u8;32])>{
        if seq==0||origin.parent.object.entity==issuer.entity||origin.parent.object.birth==issuer.birth{
            return Err(invalid("mkdir must reserve a new SYSTEM record, not its parent"));}
        let req=types::request(pool,origin.op,origin.uid,origin.parent.address,&origin.name)?;
        let mut b=MAGIC.to_vec();issuer.put(&mut b);b.extend_from_slice(&pool.0);origin.put(&mut b)?;
        b.extend_from_slice(&seq.to_be_bytes());b.extend_from_slice(&previous);b.extend_from_slice(&req);
        let digest=crate::hash::digest(&b)?;b.extend_from_slice(&digest);
        if b.len()>MAX{return Err(invalid("oversized original mkdir reservation"));}Ok((b,digest))
    }
    pub(in crate::ordinary) fn from_bytes(system:&HeldDir,pool:PoolId,stamp:fs::OriginalStamp,bytes:Vec<u8>)->io::Result<Self>{
        if bytes.len()<MAGIC.len()+32||bytes.len()>MAX{return Err(invalid("invalid mkdir reservation length"));}
        let(body,tail)=bytes.split_at(bytes.len()-32);let digest=crate::hash::digest(body)?;
        if digest.as_slice()!=tail{return Err(invalid("changed mkdir reservation bytes"));}
        let mut r=In(body.strip_prefix(MAGIC).ok_or_else(||invalid("not an original mkdir reservation"))?);
        let issuer=NativeObject::get(&mut r)?;
        if issuer!=NativeObject::stamp(&stamp)||PoolId(r.id()?)!=pool{return Err(invalid("mkdir reservation belongs to another inode or pool"));}
        let origin=Origin::get(&mut r)?;let seq=r.u64()?;let previous=r.fixed()?;let request=r.fixed()?;r.end()?;
        if seq==0||origin.parent.object.entity==issuer.entity||origin.parent.object.birth==issuer.birth||
            request!=types::request(pool,origin.op,origin.uid,origin.parent.address,&origin.name)?{
            return Err(invalid("inconsistent ordinary mkdir frontier, parent or request"));}
        let stage=allocation::stage_of(digest,&system.read_attempt_binding(allocation::NAME,&stamp,MAX)?)?;
        if let Some(st)=&stage{
            origin.check_child(st.slot,&st.object)?;
            if st.object.birth==issuer.birth||st.object.entity==issuer.entity{return Err(invalid("mkdir stage is its own SYSTEM reservation"));}
        }
        Ok(Self{pool,origin,seq,previous,request,digest,stamp,bytes,stage})
    }
    pub(in crate::ordinary) fn matches(&self,d:&Created)->bool{
        d.belongs_to(self.pool)&&d.seq==self.seq&&d.previous==self.previous&&d.request==self.request&&d.origin==self.origin&&
            d.allocation==self.digest&&self.stage.as_ref().is_some_and(|s|s.slot==d.slot&&s.object==d.object)
    }
}

#[cfg(test)]
#[path="../../../tests/unit/directory_operand.rs"]
mod tests;
