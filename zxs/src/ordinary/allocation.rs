//! A business reservation exists before any named data-volume mutation. Its
//! initially unbound SYSTEM operand can bind one real directory exactly once.
use super::{file_parent::{self,BirthParent},wire::{Frame,In,NativeObject,human_name}};
use crate::{fs::{self,HeldDir,invalid},EntryId,FileId,HistoryId,VolumeId,OperationId,PoolId};
use std::{ffi::CStr,io};
pub(super) const NAME:&CStr=c"FILE-ALLOCATION";
pub(super) const SLOTS:u8=8;
const MAGIC:&[u8]=b"ZXS/ORIGINAL-ALLOCATE-EMPTY\0";
pub(super) const MAX:usize=1024;
// Distinct commands, never an omitted parent field or two representations of
// a root birth. A child reservation alone is not a serving FILE decision.
const CHILD_MAGIC:&[u8]=b"ZXS/ORIGINAL-ALLOCATE-IN-DIRECTORY/1\0";
const NATIVE:usize=16+1+fs::MAX_BIRTH;
const MAX_CHILD_IMAGE:usize=CHILD_MAGIC.len()+2*NATIVE+7*16+4+8+32+1+super::wire::MAX_HUMAN+16+32+NATIVE+32+32;
const _:()=assert!(CHILD_MAGIC.len()>=MAGIC.len()&&MAX_CHILD_IMAGE<=MAX);
pub(super) fn root_image(bytes:&[u8])->bool{bytes.starts_with(MAGIC)}


#[derive(Clone,Debug,Eq,PartialEq)]
pub(super) struct Stage{pub slot:u8,pub object:NativeObject}
#[derive(Debug,Eq,PartialEq)]
pub(super) struct Allocation {
    pool:PoolId,pub parent:BirthParent,pub op:OperationId,pub uid:u32,pub volume:VolumeId,pub name:String,
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
/// Bounded content for one exact original SYSTEM inode. This is not an
/// installation credential: only the real initiating writer may name it.
pub(super) struct Terms<'a>{
    pub pool:PoolId,pub op:OperationId,pub uid:u32,pub volume:VolumeId,pub parent:&'a BirthParent,pub name:&'a str,
    pub seq:u64,pub previous:[u8;32],pub entry:EntryId,pub file:FileId,pub history:HistoryId,pub attempt:OperationId,pub root:&'a NativeObject,
}
impl Terms<'_>{
    pub(super) fn image(&self,issuer:&NativeObject)->io::Result<(Vec<u8>,[u8;32])>{
        let request=file_parent::request(self.pool,self.op,self.uid,self.parent.address(self.volume),self.name)?;
        require_claim(self.parent,self.root,issuer,self.op,self.attempt,self.seq)?;
        let mut bytes=match self.parent{BirthParent::Root=>MAGIC,BirthParent::Child{..}=>CHILD_MAGIC}.to_vec();issuer.put(&mut bytes);
        for id in [&self.pool.0,&self.op.0,&self.volume.0,&self.entry.0,&self.file.0,&self.history.0,&self.attempt.0]{bytes.extend_from_slice(id);}
        self.root.put(&mut bytes);bytes.extend_from_slice(&self.uid.to_be_bytes());bytes.extend_from_slice(&self.seq.to_be_bytes());
        bytes.extend_from_slice(&self.previous);bytes.push(self.name.len() as u8);bytes.extend_from_slice(self.name.as_bytes());
        self.parent.put(&mut bytes);bytes.extend_from_slice(&request);let digest=crate::hash::digest(&bytes)?;bytes.extend_from_slice(&digest);
        if bytes.len()>MAX_CHILD_IMAGE{return Err(invalid("ordinary FILE allocation exceeds its canonical limit"));}
        Ok((bytes,digest))
    }
}
fn require_claim(parent:&BirthParent,root:&NativeObject,issuer:&NativeObject,op:OperationId,attempt:OperationId,seq:u64)->io::Result<()>{
    if op==attempt||seq==0||root.entity==issuer.entity||root.birth==issuer.birth{return Err(invalid("conflicting original FILE allocation identities"));}
    if let BirthParent::Child{object,..}=parent{
        if root.entity.device!=object.entity.device||root.birth.filesystem()!=object.birth.filesystem()||
            root.entity==object.entity||root.birth==object.birth||object.entity==issuer.entity||object.birth==issuer.birth{
            return Err(invalid("not a distinct original nested parent on the declared ordinary volume"));}
    }Ok(())
}
fn require_stage(parent:&BirthParent,root:&NativeObject,issuer:&NativeObject,s:&Stage)->io::Result<()>{
    let n=&s.object;
    if s.slot>=SLOTS||n.entity.device!=root.entity.device||n.birth.filesystem()!=root.birth.filesystem()||
        n.entity==root.entity||n.birth==root.birth||n.entity==issuer.entity||n.birth==issuer.birth||
        matches!(parent,BirthParent::Child{object,..} if n.entity==object.entity||n.birth==object.birth){
        return Err(invalid("invalid original FILE stage or reused ordinary parent"));}
    Ok(())
}
impl Allocation{
    pub(super) fn reserve(system:&HeldDir,pool:PoolId,op:OperationId,volume:VolumeId,name:&str,
        seq:u64,previous:[u8;32],entry:EntryId,file:FileId,history:HistoryId,attempt:OperationId,root:NativeObject)->io::Result<()>{
        let uid=fs::local_uid();let(inode,entity)=system.anonymous_record()?;
        let origin=NativeObject{entity,birth:fs::Birth::read(&inode)?};
        // No nested writer until its entire set of operations is available.
        let terms=Terms{pool,op,uid,volume,parent:&BirthParent::Root,name,seq,previous,entry,file,history,attempt,root:&root};
        let(bytes,digest)=terms.image(&origin)?;
        fs::initial_attempt_binding(&inode,&binding(digest,None)?)?;
        system.add_record(inode,NAME,&bytes)
    }
    pub(super) fn read(system:&HeldDir,pool:PoolId)->io::Result<Option<Self>>{
        let Some((stamp,bytes))=system.record(NAME,MAX)? else{return Ok(None)};
        if !root_image(&bytes){return Err(invalid("not an issued root FILE allocation"));}
        Self::from_bytes(system,pool,stamp,bytes).map(Some)
    }
    pub(super) fn from_bytes(system:&HeldDir,pool:PoolId,stamp:fs::OriginalStamp,bytes:Vec<u8>)->io::Result<Self>{
        if bytes.len()<MAGIC.len()+32||bytes.len()>MAX_CHILD_IMAGE{return Err(invalid("invalid initial FILE allocation length"));}
        let(body,tail)=bytes.split_at(bytes.len()-32);let digest=crate::hash::digest(body)?;
        if digest.as_slice()!=tail{return Err(invalid("damaged initial FILE reservation"));}
        let(kind,rest)=if let Some(r)=body.strip_prefix(MAGIC){(file_parent::ROOT,r)}
            else if let Some(r)=body.strip_prefix(CHILD_MAGIC){(file_parent::CHILD,r)}
            else{return Err(invalid("unknown FILE allocation"));};
        let mut input=In(rest);let issuer=NativeObject::get(&mut input)?;
        if issuer!=NativeObject::stamp(&stamp)||PoolId(input.id()?)!=pool{return Err(invalid("FILE reservation is not its original SYSTEM material"));}
        let op=OperationId(input.id()?);let volume=VolumeId(input.id()?);let entry=EntryId(input.id()?);
        let file=FileId(input.id()?);let history=HistoryId(input.id()?);let attempt=OperationId(input.id()?);
        let root=NativeObject::get(&mut input)?;let uid=input.u32()?;let seq=input.u64()?;
        let previous=input.fixed()?;let name=human_name(input.short_buf()?)?.to_owned();let parent=BirthParent::get(kind,&mut input)?;
        let request=input.fixed()?;input.end()?;require_claim(&parent,&root,&issuer,op,attempt,seq)?;
        if file_parent::request(pool,op,uid,parent.address(volume),&name)?!=request{return Err(invalid("FILE allocation has another original request or parent"));}
        let stage=stage_of(digest,&system.read_attempt_binding(NAME,&stamp,MAX)?)?;
        if let Some(s)=&stage{require_stage(&parent,&root,&issuer,s)?;}
        Ok(Self{pool,parent,op,volume,entry,file,history,uid,seq,previous,name,request,attempt,root,stage,digest,stamp,bytes})
    }
    pub(super) fn require_writer(&self,h:&super::directory::path::HeldParent<'_,'_>)->io::Result<()>{
        let v=h.volume();if self.pool!=v.pool.id()||self.volume!=v.id()||
            !matches!(v.pool.access,crate::pool::lease::Access::Write)||!self.parent.identifies(v.id(),h.basis()){
            return Err(invalid("not the original FILE creation parent on this writer"));}
        let(_,s)=v.pool.snapshot()?;h.require_at(&s)?;
        if self.root.entity!=v.root.entity||self.root.birth!=fs::Birth::read(&v.root.file)?||s.allocated.as_ref()!=Some(self){
            return Err(invalid("FILE birth is not the admitted original slot and registered ordinary root"));}Ok(())
    }
    /// No detached Stage can reach the phase write. Allocate on the held
    /// parent, retain the newly-created native object across the callback,
    /// and recheck the same request, parent, namespace and object before
    /// binding. An unbound earlier empty slot is never adopted or deleted.
    pub(super) fn bind_fresh(&self,h:&super::directory::path::HeldParent<'_,'_>,after_mkdir:impl FnOnce()->io::Result<()>)->io::Result<()>{
        use super::material;
        self.require_writer(h)?;if self.stage.is_some(){return Err(invalid("original FILE ticket is already bound"));}
        // Each census already proved exactly one continuous prefix of real
        // unlabelled, empty, same-mount slots. Consume its observed names; do
        // not reopen the same private directories just to count that prefix.
        let used=|names:&[Vec<u8>]|(0..SLOTS).take_while(|&i|{let n=material::staging(self.attempt,i);
            names.iter().any(|x|x.as_slice()==n.to_bytes())}).count();
        let p=h.directory();let before=material::check_birth_name(h,&self.name,self.file,false)?;
        let slot=used(&before) as u8;if slot>=SLOTS{return Err(invalid("original ticket exhausted; preserve unbound private entries"));}
        let leaf=material::staging(self.attempt,slot);let dir=p.new_child_directory(&leaf)?;
        dir.only(None)?;dir.file.sync_all()?;p.file.sync_all()?;after_mkdir()?;
        self.require_writer(h)?;let after=material::check_birth_name(h,&self.name,self.file,false)?;
        if used(&after)!=usize::from(slot)+1{return Err(invalid("original private prefix changed before binding its new last object"));}
        let same=p.exact_directory(&leaf)?.ok_or_else(||invalid("new private FILE stage disappeared"))?;
        let object=NativeObject{entity:dir.entity,birth:fs::Birth::read(&dir.file)?};
        if same.entity!=object.entity||fs::Birth::read(&same.file)?!=object.birth||!same.same_mount(&dir)?||same.label()?.is_some(){
            return Err(invalid("new private FILE stage was replaced"));}dir.only(None)?;dir.require_named()?;
        let stage=Stage{slot,object};require_stage(&self.parent,&self.root,&NativeObject::stamp(&self.stamp),&stage)?;
        h.volume().pool.system.bind_attempt(NAME,&self.stamp,MAX,&binding(self.digest,None)?,&binding(self.digest,Some(&stage))?)
    }
    pub(super) fn bound(&self)->io::Result<&Stage>{self.stage.as_ref().ok_or_else(||invalid("original FILE directory not yet bound"))}
    pub(super) fn matches(&self,f:&Frame)->bool{
        let c=&f.create;
        f.belongs_to(self.pool)&&c.parent==self.parent&&c.allocation==self.digest&&f.seq==self.seq&&f.previous==self.previous&&f.request==self.request
            &&c.op==self.op&&c.volume==self.volume&&c.entry==self.entry&&c.file==self.file&&c.history==self.history
            &&c.uid==self.uid&&c.name==self.name&&c.attempt==self.attempt
            &&self.stage.as_ref().is_some_and(|s|s.object==c.directory&&s.slot==c.slot)
    }
}

#[cfg(test)]
#[path="../../tests/unit/file_parent_allocation.rs"]
mod scoped_tests;
