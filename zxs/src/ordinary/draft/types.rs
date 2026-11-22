//! First confirmed save is a working revision, not a formal version or node.
use crate::{FileId,HistoryId,OperationId,PoolId,VolumeId,fs::invalid};
use crate::ordinary::wire::{Frame,In,NativeObject,Envelope,encode};
use std::io;

/// A caller supplies the body length and SHA-256 before streaming. A retry that
/// already has an original result never needs the input reader again.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub struct DraftClaim{pub bytes:u64,pub sha256:[u8;32]}
pub const DRAFT_CHUNK:usize=4*1024*1024;
impl DraftClaim{
    pub fn new(bytes:u64,sha256:[u8;32])->io::Result<Self>{
        super::material::physical_size(bytes)?;
        Ok(Self{bytes,sha256})
    }
    pub fn from_hex(bytes:u64,value:&str)->io::Result<Self>{
        if value.len()!=64{return Err(invalid("expected 64 lower-case SHA-256 digits"));}
        let digit=|c:u8|match c{b'0'..=b'9'=>Ok(c-b'0'),b'a'..=b'f'=>Ok(c-b'a'+10),_=>Err(invalid("invalid SHA-256 digit"))};
        let mut result=[0;32];for (pair,out) in value.as_bytes().as_chunks::<2>().0.iter().zip(result.iter_mut()){
            *out=(digit(pair[0])?<<4)|digit(pair[1])?;
        }
        Self::new(bytes,result)
    }
}
pub(in crate::ordinary) fn request(pool:PoolId,uid:u32,op:OperationId,file:FileId,claim:DraftClaim)->io::Result<[u8;32]>{
    DraftClaim::new(claim.bytes,claim.sha256)?;
    let mut b=b"ZXS/FIRST-SAVE/REQUEST\0".to_vec();b.extend_from_slice(&pool.0);
    b.extend_from_slice(&uid.to_be_bytes());b.extend_from_slice(&op.0);b.extend_from_slice(&file.0);
    b.extend_from_slice(&0u64.to_be_bytes()); // required original working revision
    b.extend_from_slice(&claim.bytes.to_be_bytes());b.extend_from_slice(&claim.sha256);crate::hash::digest(&b)
}

#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Original{
    pub op:OperationId,pub uid:u32,pub file:FileId,pub history:HistoryId,pub volume:VolumeId,
    pub created:[u8;32],pub claim:DraftClaim,
}
impl Original{
    pub fn for_create(op:OperationId,uid:u32,c:&Frame,claim:DraftClaim)->Self{
        Self{op,uid,file:c.create.file,history:c.create.history,volume:c.create.volume,created:c.digest,claim}
    }
    pub fn matches_create(&self,c:&Frame)->bool{
        self.created==c.digest && self.file==c.create.file && self.history==c.create.history && self.volume==c.create.volume
    }
    pub fn put(&self,b:&mut Vec<u8>){
        b.extend_from_slice(&self.op.0);b.extend_from_slice(&self.uid.to_be_bytes());
        for id in [&self.file.0,&self.history.0,&self.volume.0]{b.extend_from_slice(id);}
        b.extend_from_slice(&self.created);b.extend_from_slice(&self.claim.bytes.to_be_bytes());b.extend_from_slice(&self.claim.sha256);
    }
    pub fn get(r:&mut In<'_>)->io::Result<Self>{
        let op=OperationId(r.id()?);let uid=r.u32()?;let file=FileId(r.id()?);
        let history=HistoryId(r.id()?);let volume=VolumeId(r.id()?);let created=r.fixed()?;
        let claim=DraftClaim::new(r.u64()?,r.fixed()?)?;
        Ok(Self{op,uid,file,history,volume,created,claim})
    }
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Material{pub object:NativeObject,pub chain:[u8;32]}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct SaveFrame{
    pub seq:u64,pub previous:[u8;32],pub request:[u8;32],pub digest:[u8;32],pub bytes:Vec<u8>,
    pub original:Original,pub allocation:[u8;32],pub material:Material,
}
impl SaveFrame{
    pub fn new(pool:PoolId,seq:u64,prev:[u8;32],original:Original,allocation:[u8;32],material:Material)->io::Result<Self>{
        let req=request(pool,original.uid,original.op,original.file,original.claim)?;
        let mut body=Vec::new();original.put(&mut body);body.extend_from_slice(&allocation);
        material.object.put(&mut body);body.extend_from_slice(&material.chain);
        let (bytes,digest)=encode(pool,seq,prev,2,req,&body)?;
        Ok(Self{seq,previous:prev,request:req,digest,bytes,original,allocation,material})
    }
    pub fn from_envelope(pool:PoolId,bytes:&[u8],e:Envelope<'_>)->io::Result<Self>{
        if e.kind!=2{return Err(invalid("not an original first-save frame"));}
        let mut r=In(e.body);let original=Original::get(&mut r)?;
        let allocation=r.fixed()?;let material=Material{object:NativeObject::get(&mut r)?,chain:r.fixed()?};r.end()?;
        if request(pool,original.uid,original.op,original.file,original.claim)?!=e.request{return Err(invalid("first-save request digest differs"));}
        Ok(Self{seq:e.seq,previous:e.previous,request:e.request,digest:e.digest,bytes:bytes.to_vec(),original,allocation,material})
    }
    pub fn receipt(&self)->DraftReceipt{
        DraftReceipt{file:self.original.file,operation:self.original.op,revision:1,decision_sequence:self.seq,body:self.original.claim}
    }
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub struct DraftReceipt{
    pub file:FileId,pub operation:OperationId,pub revision:u64,pub decision_sequence:u64,pub body:DraftClaim,
}
