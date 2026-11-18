//! One original checked logical reservation, in the same SYSTEM slot used
//! by any other FILE operation. It owns no serving bytes or separate vote.
use super::types::{Original,Commit,request};
use crate::{PoolId,fs::{self,HeldDir,invalid}};
use crate::ordinary::{allocation::NAME,wire::{In,NativeObject}};
use std::io;
pub(in crate::ordinary) const MAGIC:&[u8]=b"ZXS/ORIGINAL-FIRST-FORMAL\0";
pub(in crate::ordinary) struct FormalAllocation {
    pub original:Original,pub request:[u8;32],pub seq:u64,pub previous:[u8;32],
    pub digest:[u8;32],pub stamp:fs::OriginalStamp,pub bytes:Vec<u8>,
}
impl FormalAllocation {
    pub fn reserve(sys:&HeldDir,pool:PoolId,original:Original,seq:u64,previous:[u8;32])->io::Result<()>{
        let req=request(pool,original.uid,original.op,original.file,1)?;
        let (f,entity)=sys.anonymous_record()?;let mut b=MAGIC.to_vec();
        NativeObject{entity,birth:fs::Birth::read(&f)?}.put(&mut b);
        b.extend_from_slice(&pool.0);original.put(&mut b);b.extend_from_slice(&seq.to_be_bytes());
        b.extend_from_slice(&previous);b.extend_from_slice(&req);let sha=crate::hash::digest(&b)?;b.extend_from_slice(&sha);
        sys.add_record(f,NAME,&b)
    }
    pub fn from_bytes(pool:PoolId,stamp:fs::OriginalStamp,bytes:Vec<u8>)->io::Result<Self>{
        if bytes.len()<MAGIC.len()+32{return Err(invalid("short explicit commit reservation"));}
        let (body,sum)=bytes.split_at(bytes.len()-32);let digest=crate::hash::digest(body)?;
        if digest.as_slice()!=sum{return Err(invalid("damaged original explicit commit reservation"));}
        let mut r=In(body.strip_prefix(MAGIC).ok_or_else(||invalid("unknown explicit commit reservation"))?);
        if NativeObject::get(&mut r)?!=NativeObject::stamp(&stamp)||PoolId(r.id()?)!=pool{return Err(invalid("not the original SYSTEM explicit commit reservation"));}
        let original=Original::get(&mut r)?;let seq=r.u64()?;let previous=r.fixed()?;let req=r.fixed()?;r.end()?;
        if seq==0||request(pool,original.uid,original.op,original.file,1)?!=req{return Err(invalid("explicit commit reservation recipe differs"));}
        Ok(Self{original,seq,previous,request:req,digest,stamp,bytes})
    }
    pub fn matches(&self,f:&Commit)->bool{
        self.seq==f.seq && self.previous==f.previous && self.request==f.request && self.original==f.original && self.digest==f.allocation
    }
}
