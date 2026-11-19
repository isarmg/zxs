use crate::{EntryId,FileId,HistoryId,OperationId,PoolId,VolumeId,Entity,fs::{self,Birth,invalid}};
use super::file_parent::{self,BirthParent};
use std::io;

pub(super) struct In<'a>(pub(super) &'a[u8]);
impl<'a> In<'a>{
    pub(super) fn take(&mut self,n:usize)->io::Result<&'a[u8]>{
        let (v,rest)=self.0.split_at_checked(n).ok_or_else(||invalid("short native FILE record"))?;
        self.0=rest;Ok(v)
    }
    pub(super) fn fixed<const N:usize>(&mut self)->io::Result<[u8;N]>{
        Ok(self.take(N)?.try_into().expect("exact fixed width"))
    }
    pub(super) fn id(&mut self)->io::Result<[u8;16]>{
        let id=self.fixed()?;
        if id==[0;16]{return Err(invalid("empty typed FILE identity"));}
        Ok(id)
    }
    pub(super) fn byte(&mut self)->io::Result<u8>{Ok(self.take(1)?[0])}
    pub(super) fn u32(&mut self)->io::Result<u32>{Ok(u32::from_be_bytes(self.fixed()?))}
    pub(super) fn u64(&mut self)->io::Result<u64>{Ok(u64::from_be_bytes(self.fixed()?))}
    pub(super) fn short_buf(&mut self)->io::Result<&'a[u8]>{let n=usize::from(self.byte()?);self.take(n)}
    pub(super) fn end(self)->io::Result<()>{if self.0.is_empty(){Ok(())}else{Err(invalid("unexpected trailing FILE record data"))}}
}

#[derive(Clone,Debug,Eq,PartialEq)]
pub(crate) struct NativeObject{ pub entity:Entity,pub birth:Birth }
impl NativeObject{
    pub(super) fn put(&self,out:&mut Vec<u8>){
        out.extend_from_slice(&self.entity.device.to_be_bytes());out.extend_from_slice(&self.entity.inode.to_be_bytes());
        out.push(self.birth.bytes().len() as u8);out.extend_from_slice(self.birth.bytes());
    }
    pub(super) fn get(input:&mut In<'_>)->io::Result<Self>{
        let entity=Entity{device:input.u64()?,inode:input.u64()?};
        if entity.inode==0{return Err(invalid("missing physical FILE entity"));}
        Ok(Self{entity,birth:Birth::decode(input.short_buf()?)?})
    }
    pub(super) fn stamp(s:&fs::OriginalStamp)->Self{Self{entity:s.stat.entity(),birth:s.birth.clone()}}
}

#[derive(Clone,Debug,Eq,PartialEq)]
pub(crate) struct Creation {
    pub(super) parent:BirthParent,pub op:OperationId,pub uid:u32,pub volume:VolumeId,pub entry:EntryId,pub file:FileId,pub history:HistoryId,
    pub name:String,pub attempt:OperationId,pub slot:u8,pub allocation:[u8;32],pub directory:NativeObject,pub local:NativeObject,pub local_digest:[u8;32],
}
pub(super) const MAX_HUMAN:usize=super::storage_name::FILE_NAME_MAX;
pub(super) fn human_name(bytes:&[u8])->io::Result<&str>{super::storage_name::FileName::parse(bytes).map(|n|n.display())}
pub(super) fn request_digest(pool:PoolId,op:OperationId,uid:u32,volume:VolumeId,name:&str)->io::Result<[u8;32]>{
    human_name(name.as_bytes())?;
    let mut b=b"ZXS/CREATE-EMPTY-REQUEST\0".to_vec();
    b.extend_from_slice(&pool.0);b.extend_from_slice(&op.0);b.extend_from_slice(&uid.to_be_bytes());
    b.extend_from_slice(&volume.0);b.push(name.len() as u8);b.extend_from_slice(name.as_bytes());crate::hash::digest(&b)
}
pub(super) fn empty_material(file:FileId,history:HistoryId,op:OperationId,name:&str)->Vec<u8>{
    // Zero confirmed revisions, zero formal nodes and zero logical bytes.
    // The owner's physical volume/entity does not live inside immutable FILE history.
    let mut b=b"ZXS/FILE/UNBORN-MANUAL\0".to_vec();
    b.extend_from_slice(&file.0);b.extend_from_slice(&history.0);b.extend_from_slice(&op.0);
    b.push(name.len() as u8);b.extend_from_slice(name.as_bytes());b.extend_from_slice(&0u64.to_be_bytes());b
}
impl Creation{
    fn put(&self,out:&mut Vec<u8>){
        out.extend_from_slice(&self.op.0);out.extend_from_slice(&self.uid.to_be_bytes());
        for id in [&self.volume.0,&self.entry.0,&self.file.0,&self.history.0,&self.attempt.0]{out.extend_from_slice(id);}
        out.push(self.name.len() as u8);out.extend_from_slice(self.name.as_bytes());
        out.push(self.slot);out.extend_from_slice(&self.allocation);self.directory.put(out);self.local.put(out);out.extend_from_slice(&self.local_digest);self.parent.put(out);
    }
    fn get(kind:u8,input:&mut In<'_>)->io::Result<Self>{
        let op=OperationId(input.id()?);let uid=input.u32()?;let volume=VolumeId(input.id()?);
        let entry=EntryId(input.id()?);let file=FileId(input.id()?);let history=HistoryId(input.id()?);
        let attempt=OperationId(input.id()?);let name=human_name(input.short_buf()?)?.to_owned();
        let slot=input.byte()?;
        if slot>=super::allocation::SLOTS{return Err(invalid("unknown FILE creation attempt ordinal"));}
        let allocation=input.fixed()?;let directory=NativeObject::get(input)?;let local=NativeObject::get(input)?;let local_digest=input.fixed()?;
        if directory.entity.device!=local.entity.device || directory.entity==local.entity ||
            directory.birth.filesystem()!=local.birth.filesystem() || directory.birth==local.birth ||
            crate::hash::digest(&empty_material(file,history,op,&name))?!=local_digest{
            return Err(invalid("inconsistent original empty FILE material"));
        }
        let parent=BirthParent::get(kind,input)?;parent.check_object(&directory,&local)?;
        Ok(Self{parent,op,uid,volume,entry,file,history,name,attempt,slot,allocation,directory,local,local_digest})
    }
}
const MAGIC:&[u8]=b"ZXS/FILE-DECISION\0";
pub(super) const MAX_FRAME:usize=4096;
pub(super) const ENVELOPE_BYTES:usize=MAGIC.len()+4+16+8+32+1+32+32;
const NATIVE:usize=16+1+fs::MAX_BIRTH;
const MAX_CREATE_BODY:usize=16+4+5*16+1+MAX_HUMAN+1+32+2*NATIVE+32;
const MAX_CHILD_BIRTH:usize=ENVELOPE_BYTES+MAX_CREATE_BODY+16+32+NATIVE;
const _:()=assert!(MAX_CHILD_BIRTH<=MAX_FRAME);

#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) struct Frame{
    pool:PoolId,pub seq:u64,pub previous:[u8;32],pub create:Creation,pub request:[u8;32],pub digest:[u8;32],pub bytes:Vec<u8>,
}
pub(super) struct Envelope<'a>{
    pub seq:u64,pub previous:[u8;32],pub kind:u8,pub request:[u8;32],pub body:&'a[u8],pub digest:[u8;32],
}
pub(super) fn encode(pool:PoolId,seq:u64,prev:[u8;32],kind:u8,request:[u8;32],body:&[u8])->io::Result<(Vec<u8>,[u8;32])>{
    let mut bytes=MAGIC.to_vec();bytes.extend_from_slice(&[0;4]);bytes.extend_from_slice(&pool.0);
    bytes.extend_from_slice(&seq.to_be_bytes());bytes.extend_from_slice(&prev);bytes.push(kind);
    bytes.extend_from_slice(&request);bytes.extend_from_slice(body);
    let n=bytes.len()+32;if n>MAX_FRAME||body.len().checked_add(ENVELOPE_BYTES)!=Some(n){return Err(invalid("FILE decision exceeds or differs from its frame budget"));}
    bytes[MAGIC.len()..MAGIC.len()+4].copy_from_slice(&(n as u32).to_be_bytes());
    let digest=crate::hash::digest(&bytes)?;bytes.extend_from_slice(&digest);Ok((bytes,digest))
}
pub(super) fn envelope(pool:PoolId,bytes:&[u8])->io::Result<Envelope<'_>>{
    if Frame::size(bytes)?!=Some(bytes.len()){return Err(invalid("incomplete SYSTEM decision"));}
    let (body,hash)=bytes.split_at(bytes.len()-32);
    let digest=crate::hash::digest(body)?;if digest.as_slice()!=hash{return Err(invalid("SYSTEM decision checksum mismatch"));}
    let mut input=In(&body[MAGIC.len()+4..]);if PoolId(input.id()?)!=pool{return Err(invalid("foreign pool decision"));}
    let seq=input.u64()?;let previous=input.fixed()?;let kind=input.byte()?;let request=input.fixed()?;
    if seq==0{return Err(invalid("zero SYSTEM decision sequence"));}
    Ok(Envelope{seq,previous,kind,request,body:input.0,digest})
}
impl Frame{
    pub(super) fn belongs_to(&self,pool:PoolId)->bool{self.pool==pool}
    pub(super) fn size(bytes:&[u8])->io::Result<Option<usize>>{
        if !MAGIC.starts_with(&bytes[..bytes.len().min(MAGIC.len())]){return Err(invalid("unknown SYSTEM frame prefix"));}
        if bytes.len()<MAGIC.len()+4{return Ok(None);}
        let n=u32::from_be_bytes(bytes[MAGIC.len()..MAGIC.len()+4].try_into().expect("four length bytes")) as usize;
        if !(MAGIC.len()+4+32+8+32+16+1+32..=MAX_FRAME).contains(&n){return Err(invalid("invalid SYSTEM frame size"));}
        Ok(Some(n))
    }
    pub(super) fn new(pool:PoolId,seq:u64,previous:[u8;32],c:Creation)->io::Result<Self>{
        c.parent.check_object(&c.directory,&c.local)?;
        let request=file_parent::request(pool,c.op,c.uid,c.parent.address(c.volume),&c.name)?;
        let mut body=Vec::new();c.put(&mut body);
        let (bytes,digest)=encode(pool,seq,previous,c.parent.kind(),request,&body)?;
        Ok(Self{pool,seq,previous,create:c,request,digest,bytes})
    }
    pub(super) fn read(pool:PoolId,bytes:&[u8])->io::Result<Self>{
        Self::from_envelope(pool,bytes,envelope(pool,bytes)?)
    }
    pub(super) fn from_envelope(pool:PoolId,bytes:&[u8],e:Envelope<'_>)->io::Result<Self>{
        if !matches!(e.kind,file_parent::ROOT|file_parent::CHILD)||bytes.len()>MAX_CHILD_BIRTH{return Err(invalid("not a bounded FILE creation"));}
        let (seq,previous,request,digest)=(e.seq,e.previous,e.request,e.digest);
        let mut input=In(e.body);let create=Creation::get(e.kind,&mut input)?;input.end()?;
        if file_parent::request(pool,create.op,create.uid,create.parent.address(create.volume),&create.name)?!=request {
            return Err(invalid("FILE decision does not match original request"));
        }
        Ok(Self{pool,seq,previous,create,request,digest,bytes:bytes.to_vec()})
    }
}
