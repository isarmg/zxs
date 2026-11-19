//! A business reservation exists before any named data-volume mutation. Its
//! initially unbound SYSTEM operand can bind one real directory exactly once.
use super::wire::{Frame,In,NativeObject,request_digest,human_name};
use crate::{fs::{self,HeldDir,invalid},EntryId,FileId,HistoryId,VolumeId,OperationId,PoolId};
use std::{ffi::CStr,io};
pub(super) const NAME:&CStr=c"FILE-ALLOCATION";
pub(super) const SLOTS:u8=8;
const MAGIC:&[u8]=b"ZXS/ORIGINAL-ALLOCATE-EMPTY\0";
pub(super) const MAX:usize=1024;

#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) struct Stage{pub slot:u8,pub object:NativeObject}
pub(super) struct Allocation {
    pub op:OperationId,pub uid:u32,pub volume:VolumeId,pub name:String,
    pub entry:EntryId,pub file:FileId,pub history:HistoryId,pub previous:[u8;32],pub seq:u64,pub request:[u8;32],
    pub attempt:OperationId,pub root:NativeObject,pub stage:Option<Stage>,pub digest:[u8;32],pub stamp:fs::OriginalStamp,pub bytes:Vec<u8>,
}

pub(super) fn binding(digest:[u8;32],stage:Option<&Stage>)->io::Result<Vec<u8>>{
    let mut bytes=Vec::new();
    match stage{None=>bytes.push(0),Some(s)=>{
        if s.slot>=SLOTS{return Err(invalid("attempt ordinal out of range"));}
        bytes.extend_from_slice(&[1,s.slot]);s.object.put(&mut bytes);
    }}
    let mut hash=crate::hash::Sha256::new();hash.update(b"ZXS/ORIGINAL-STAGE-BINDING\0")?;
    hash.update(&digest)?;hash.update(&bytes)?;bytes.extend_from_slice(&hash.finish());Ok(bytes)
}
pub(super) fn stage_of(digest:[u8;32],bytes:&[u8])->io::Result<Option<Stage>>{
    let mut r=In(bytes);let stage=match r.byte()?{
        0=>None,
        1=>Some(Stage{slot:r.byte()?,object:NativeObject::get(&mut r)?}),
        _=>return Err(invalid("unrecognized original FILE stage state")),
    };
    // Canonical digest, no unknown tail, missing attribute is never unbound.
    if binding(digest,stage.as_ref())?.as_slice()!=bytes{return Err(invalid("invalid original FILE stage binding"));}
    Ok(stage)
}
impl Allocation{
    pub(super) fn reserve(system:&HeldDir,pool:PoolId,op:OperationId,volume:VolumeId,name:&str,
        seq:u64,previous:[u8;32],entry:EntryId,file:FileId,history:HistoryId,attempt:OperationId,root:NativeObject)->io::Result<()>{
        let uid=fs::local_uid();let request=request_digest(pool,op,uid,volume,name)?;
        let (inode,entity)=system.anonymous_record()?;
        let origin=NativeObject{entity,birth:fs::Birth::read(&inode)?};
        let mut bytes=MAGIC.to_vec();origin.put(&mut bytes);
        for id in [&pool.0,&op.0,&volume.0,&entry.0,&file.0,&history.0,&attempt.0]{bytes.extend_from_slice(id);}
        root.put(&mut bytes);bytes.extend_from_slice(&uid.to_be_bytes());bytes.extend_from_slice(&seq.to_be_bytes());
        bytes.extend_from_slice(&previous);bytes.push(name.len() as u8);bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&request);let digest=crate::hash::digest(&bytes)?;
        bytes.extend_from_slice(&digest);
        fs::initial_attempt_binding(&inode,&binding(digest,None)?)?;
        system.add_record(inode,NAME,&bytes)
    }
    pub(super) fn read(system:&HeldDir,pool:PoolId)->io::Result<Option<Self>>{
        let Some((stamp,bytes))=system.record(NAME,MAX)? else{return Ok(None)};
        Self::from_bytes(system,pool,stamp,bytes).map(Some)
    }
    pub(super) fn from_bytes(system:&HeldDir,pool:PoolId,stamp:fs::OriginalStamp,bytes:Vec<u8>)->io::Result<Self>{
        if bytes.len()<MAGIC.len()+32{return Err(invalid("short initial FILE reservation"));}
        let (body,tail)=bytes.split_at(bytes.len()-32);
        let digest=crate::hash::digest(body)?;
        if digest.as_slice()!=tail{return Err(invalid("damaged initial FILE reservation"));}
        let mut input=In(body.strip_prefix(MAGIC).ok_or_else(||invalid("unknown FILE allocation"))?);
        if NativeObject::get(&mut input)?!=NativeObject::stamp(&stamp)||PoolId(input.id()?)!=pool{
            return Err(invalid("FILE reservation is not its original SYSTEM material"));
        }
        let op=OperationId(input.id()?);let volume=VolumeId(input.id()?);let entry=EntryId(input.id()?);
        let file=FileId(input.id()?);let history=HistoryId(input.id()?);let attempt=OperationId(input.id()?);
        let root=NativeObject::get(&mut input)?;let uid=input.u32()?;let seq=input.u64()?;
        let previous=input.fixed()?;let name=human_name(input.short_buf()?)?.to_owned();let request=input.fixed()?;input.end()?;
        let stage=stage_of(digest,&system.read_attempt_binding(NAME,&stamp,MAX)?)?;
        if seq==0||request_digest(pool,op,uid,volume,&name)?!=request||
            stage.as_ref().is_some_and(|s|s.object.entity.device!=root.entity.device||s.object.birth.filesystem()!=root.birth.filesystem()
                ||s.object.entity==root.entity||s.object.birth==root.birth){
            return Err(invalid("FILE reservation request, owner or frontier differs"));
        }
        Ok(Self{op,volume,entry,file,history,uid,seq,previous,name,request,attempt,root,stage,digest,stamp,bytes})
    }
    pub(super) fn bind(&self,system:&HeldDir,stage:&Stage)->io::Result<()>{
        if self.stage.is_some(){return Err(invalid("an original FILE directory is already bound"));}
        system.bind_attempt(NAME,&self.stamp,MAX,&binding(self.digest,None)?,&binding(self.digest,Some(stage))?)
    }
    pub(super) fn bound(&self)->io::Result<&Stage>{self.stage.as_ref().ok_or_else(||invalid("original FILE directory not yet bound"))}
    pub(super) fn matches(&self,f:&Frame)->bool{
        let c=&f.create;
        c.allocation==self.digest&&f.seq==self.seq&&f.previous==self.previous&&f.request==self.request
            &&c.op==self.op&&c.volume==self.volume&&c.entry==self.entry&&c.file==self.file&&c.history==self.history
            &&c.uid==self.uid&&c.name==self.name&&c.attempt==self.attempt
            &&self.stage.as_ref().is_some_and(|s|s.object==c.directory&&s.slot==c.slot)
    }
}
