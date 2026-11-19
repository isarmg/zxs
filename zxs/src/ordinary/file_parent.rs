//! A FILE creation has exactly one semantic parent. Root is conveyed by the
//! original root-only opcode (and is never a missing child field). A nested
//! parent is decoded only as such and cannot by itself authorize physical I/O.
use super::{directory::types::Parent,wire::{self,In,NativeObject}};
use crate::{DirectoryId,OperationId,PoolId,VolumeId,fs::invalid};
use std::io;

pub(super) const ROOT:u8=1;
pub(super) const CHILD:u8=8;
#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) enum BirthParent{
    Root,
    Child{directory:DirectoryId,decision:[u8;32],object:NativeObject},
}
impl BirthParent{
    pub(super) fn address(&self,volume:VolumeId)->Parent{match self{
        Self::Root=>Parent::Root(volume),Self::Child{directory,..}=>Parent::Child(volume,*directory),
    }}
    pub(super) fn kind(&self)->u8{match self{Self::Root=>ROOT,Self::Child{..}=>CHILD}}
    pub(super) fn put(&self,b:&mut Vec<u8>){
        if let Self::Child{directory,decision,object}=self{b.extend_from_slice(&directory.0);b.extend_from_slice(decision);object.put(b);}
    }
    pub(super) fn get(kind:u8,r:&mut In<'_>)->io::Result<Self>{match kind{
        ROOT=>Ok(Self::Root),
        CHILD=>Ok(Self::Child{directory:DirectoryId(r.id()?),decision:r.fixed()?,object:NativeObject::get(r)?}),
        _=>Err(invalid("not a root or child FILE creation")),
    }}
    pub(super) fn check_object(&self,container:&NativeObject,local:&NativeObject)->io::Result<()>{
        if let Self::Child{object:p,..}=self{
            for x in [container,local]{if p.entity==x.entity||p.birth==x.birth||p.entity.device!=x.entity.device||
                p.birth.filesystem()!=x.birth.filesystem(){return Err(invalid("FILE materials are not distinct originals on the ordinary parent's filesystem"));}}
        }Ok(())
    }
}
/// The caller chooses the typed parent; the original native parent and its
/// birth decision are separately fixed by the original operand. A completed
/// receipt can match this request without opening the data volume again.
pub(super) fn request(pool:PoolId,op:OperationId,uid:u32,address:Parent,name:&str)->io::Result<[u8;32]>{
    if let Parent::Root(v)=address{return wire::request_digest(pool,op,uid,v,name);}
    wire::human_name(name.as_bytes())?;
    let mut b=b"ZXS/FILE/CREATE-IN-DIRECTORY/1\0".to_vec();
    b.extend_from_slice(&pool.0);b.extend_from_slice(&op.0);b.extend_from_slice(&uid.to_be_bytes());
    address.put(&mut b);b.push(name.len() as u8);b.extend_from_slice(name.as_bytes());crate::hash::digest(&b)
}

#[cfg(test)]
#[path="../../tests/unit/file_birth_parent.rs"]
mod tests;
