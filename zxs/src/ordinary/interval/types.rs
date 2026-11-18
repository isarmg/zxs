//! One explicitly saved edit over an exact formal or already-saved working revision. The stamped SHA
//! is for the supplied extent, never a fabricated whole-output hash.
use crate::{FileId,HistoryId,BranchId,VersionId,NodeId,VolumeId,PoolId,OperationId,fs::invalid};
use crate::ordinary::{draft::types::DraftClaim,formal::types::Commit as Base,wire::{In,Envelope,NativeObject,encode}};
use crate::ordinary::placement::Placement;
use std::io;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub enum LengthPolicy{KeepOrGrow,Set(u64)}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub struct SpanEdit{pub offset:u64,pub input:DraftClaim,pub length:LengthPolicy}
impl SpanEdit{
    pub fn new(offset:u64,input:DraftClaim,length:LengthPolicy)->io::Result<Self>{
        DraftClaim::new(input.bytes,input.sha256)?;
        let end=offset.checked_add(input.bytes).filter(|n|*n<=u64::MAX/8).ok_or_else(||invalid("changed interval overflows"))?;
        if let LengthPolicy::Set(n)=length{
            if n<end||n>u64::MAX/8{return Err(invalid("explicit final length does not contain the entire write"));}
        }
        if input.bytes==0&&(offset!=0||matches!(length,LengthPolicy::KeepOrGrow)||input.sha256!=crate::hash::digest(b"")?){
            return Err(invalid("a resize without input requires offset zero, explicit length and the empty digest"));
        }
        Ok(Self{offset,input,length})
    }
    pub(in crate::ordinary) fn end(self)->u64{self.offset+self.input.bytes}
    pub(in crate::ordinary) fn result(self,old:u64)->io::Result<u64>{
        Self::new(self.offset,self.input,self.length)?;
        let result=match self.length{LengthPolicy::KeepOrGrow=>old.max(self.end()),LengthPolicy::Set(n)=>n};
        if self.input.bytes==0&&result==old{return Err(invalid("zero-byte resize has no effect"));}
        Ok(result)
    }
    fn put(self,b:&mut Vec<u8>){
        b.extend_from_slice(&self.offset.to_be_bytes());b.extend_from_slice(&self.input.bytes.to_be_bytes());b.extend_from_slice(&self.input.sha256);
        match self.length{LengthPolicy::KeepOrGrow=>b.push(0),LengthPolicy::Set(n)=>{b.push(1);b.extend_from_slice(&n.to_be_bytes());}}
    }
    fn get(r:&mut In<'_>)->io::Result<Self>{
        let off=r.u64()?;let input=DraftClaim::new(r.u64()?,r.fixed()?)?;
        let length=match r.byte()?{0=>LengthPolicy::KeepOrGrow,1=>LengthPolicy::Set(r.u64()?),_=>return Err(invalid("unknown working length policy"))};
        Self::new(off,input,length)
    }
}
pub(in crate::ordinary) fn request(pool:PoolId,uid:u32,op:OperationId,file:FileId,rev:u64,e:SpanEdit)->io::Result<[u8;32]>{
    if rev==0||rev==u64::MAX{return Err(invalid("not a supported confirmed predecessor working revision"));}
    SpanEdit::new(e.offset,e.input,e.length)?;
    let mut b=b"ZXS/WORKING/FORMAL-BASE/REQUEST\0".to_vec();b.extend_from_slice(&pool.0);b.extend_from_slice(&uid.to_be_bytes());
    b.extend_from_slice(&op.0);b.extend_from_slice(&file.0);b.extend_from_slice(&rev.to_be_bytes());e.put(&mut b);crate::hash::digest(&b)
}
/// The immediately preceding FILE revision, not the previous global frame.
/// All four fields are checked against an admitted original before use.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Predecessor{pub op:OperationId,pub decision:[u8;32],pub revision:u64,pub length:u64}
impl Predecessor{
    fn actual(f:&Base,previous:Option<&Saved>)->Self{
        match previous{
            Some(s)=>Self{op:s.original.op,decision:s.digest,revision:s.original.revision(),length:s.original.result},
            None=>Self{op:f.original.op,decision:f.digest,revision:1,length:f.original.content.bytes},
        }
    }
    fn put(self,b:&mut Vec<u8>){
        b.extend_from_slice(&self.op.0);b.extend_from_slice(&self.decision);
        b.extend_from_slice(&self.revision.to_be_bytes());b.extend_from_slice(&self.length.to_be_bytes());
    }
    fn get(r:&mut In<'_>)->io::Result<Self>{
        let v=Self{op:OperationId(r.id()?),decision:r.fixed()?,revision:r.u64()?,length:r.u64()?};
        if v.revision==0||v.revision==u64::MAX||v.length>u64::MAX/8{return Err(invalid("invalid prior working revision or size"));}
        Ok(v)
    }
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Original{
    pub op:OperationId,pub uid:u32,pub file:FileId,pub history:HistoryId,pub volume:VolumeId,pub birth:[u8;32],
    pub commit:[u8;32],pub branch:BranchId,pub node:NodeId,pub version:VersionId,pub base_len:u64,
    pub predecessor:Predecessor,pub edit:SpanEdit,pub result:u64,
}
impl Original{
    pub fn select(op:OperationId,uid:u32,place:Placement<'_>,f:&Base,previous:Option<&Saved>,revision:u64,edit:SpanEdit)->io::Result<Self>{
        let c=place.birth();
        if previous.is_some_and(|e|e.original.volume!=place.volume())||f.original.file!=c.create.file||f.original.creation!=c.digest{return Err(invalid("different original FILE for interval"));}
        let predecessor=Predecessor::actual(f,previous);
        if revision!=predecessor.revision||revision==u64::MAX{return Err(invalid("working revision changed; refusing stale write"));}
        Ok(Self{op,uid,file:c.create.file,history:c.create.history,volume:place.volume(),birth:c.digest,commit:f.digest,
            branch:f.original.branch,node:f.original.node,version:f.original.version,base_len:f.original.content.bytes,predecessor,edit,
            result:edit.result(predecessor.length)?})
    }
    pub fn revision(&self)->u64{self.predecessor.revision+1}
    pub fn matches(&self,place:Placement<'_>,f:&Base,previous:Option<&Saved>)->bool{
        let c=place.birth();
        self.file==c.create.file&&self.history==c.create.history&&self.volume==place.volume()&&self.birth==c.digest&&
            self.commit==f.digest&&self.file==f.original.file&&self.branch==f.original.branch&&self.node==f.original.node&&
            self.version==f.original.version&&self.base_len==f.original.content.bytes&&
            !previous.is_some_and(|e|e.original.volume!=place.volume())&&self.predecessor==Predecessor::actual(f,previous)&&self.edit.result(self.predecessor.length).ok()==Some(self.result)
    }
    pub fn put(&self,b:&mut Vec<u8>){
        for id in [&self.op.0,&self.file.0,&self.history.0,&self.volume.0]{b.extend_from_slice(id);}
        b.extend_from_slice(&self.uid.to_be_bytes());b.extend_from_slice(&self.birth);b.extend_from_slice(&self.commit);
        for id in [&self.branch.0,&self.node.0,&self.version.0]{b.extend_from_slice(id);}
        b.extend_from_slice(&self.base_len.to_be_bytes());self.predecessor.put(b);self.edit.put(b);b.extend_from_slice(&self.result.to_be_bytes());
    }
    pub fn get(r:&mut In<'_>)->io::Result<Self>{
        let op=OperationId(r.id()?);let file=FileId(r.id()?);let history=HistoryId(r.id()?);let volume=VolumeId(r.id()?);
        let uid=r.u32()?;let birth=r.fixed()?;let commit=r.fixed()?;let branch=BranchId(r.id()?);let node=NodeId(r.id()?);
        let version=VersionId(r.id()?);let base_len=r.u64()?;let predecessor=Predecessor::get(r)?;let edit=SpanEdit::get(r)?;let result=r.u64()?;
        if base_len>u64::MAX/8||edit.result(predecessor.length)?!=result{return Err(invalid("conflicting interval length or original baseline"));}
        Ok(Self{op,uid,file,history,volume,birth,commit,branch,node,version,base_len,predecessor,edit,result})
    }
    pub fn header(&self,chain:[u8;32])->Vec<u8>{
        let mut b=b"ZXS/FILE/WORKING/AFTER-FORMAL\0".to_vec();
        for id in [&self.op.0,&self.file.0,&self.history.0,&self.branch.0,&self.node.0,&self.version.0]{b.extend_from_slice(id);}
        b.extend_from_slice(&self.predecessor.op.0);
        for val in [self.predecessor.revision,self.revision(),self.base_len,self.predecessor.length,self.result]{b.extend_from_slice(&val.to_be_bytes());}
        self.edit.put(&mut b);b.extend_from_slice(&chain);b
    }
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Material{pub object:NativeObject,pub chain:[u8;32],pub index:[u8;32]}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Saved{
    pub seq:u64,pub previous:[u8;32],pub request:[u8;32],pub digest:[u8;32],pub bytes:Vec<u8>,
    pub original:Original,pub allocation:[u8;32],pub material:Material,
}
impl Saved{
    pub fn new(pool:PoolId,seq:u64,previous:[u8;32],original:Original,allocation:[u8;32],material:Material)->io::Result<Self>{
        let request=request(pool,original.uid,original.op,original.file,original.predecessor.revision,original.edit)?;let mut b=Vec::new();
        original.put(&mut b);b.extend_from_slice(&allocation);material.object.put(&mut b);b.extend_from_slice(&material.chain);b.extend_from_slice(&material.index);
        let (bytes,digest)=encode(pool,seq,previous,4,request,&b)?;Ok(Self{seq,previous,request,digest,bytes,original,allocation,material})
    }
    pub fn from_envelope(pool:PoolId,bytes:&[u8],e:Envelope<'_>)->io::Result<Self>{
        if e.kind!=4{return Err(invalid("not a working interval decision"));}
        let mut r=In(e.body);let original=Original::get(&mut r)?;let allocation=r.fixed()?;
        let material=Material{object:NativeObject::get(&mut r)?,chain:r.fixed()?,index:r.fixed()?};r.end()?;
        if request(pool,original.uid,original.op,original.file,original.predecessor.revision,original.edit)?!=e.request{return Err(invalid("original working interval request differs"));}
        Ok(Self{seq:e.seq,previous:e.previous,request:e.request,digest:e.digest,bytes:bytes.to_vec(),original,allocation,material})
    }
    pub fn receipt(&self)->SpanReceipt{
        let o=&self.original;SpanReceipt{file:o.file,operation:o.op,revision:o.revision(),base_node:o.node,base_version:o.version,
            source_revision:o.predecessor.revision,edit:o.edit,logical_bytes:o.result,decision_sequence:self.seq}
    }
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub struct SpanReceipt{pub file:FileId,pub operation:OperationId,pub revision:u64,pub base_node:NodeId,pub base_version:VersionId,
    pub source_revision:u64,pub edit:SpanEdit,pub logical_bytes:u64,pub decision_sequence:u64}
