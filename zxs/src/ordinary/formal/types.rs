//! A real first main-branch content node with no parents. No namespace
//! operation, draft SAVE or same-content lookup can synthesize this command.
use crate::{BranchId,NodeId,VersionId,VersionStorageId,FileId,HistoryId,OperationId,PoolId,VolumeId,fs::invalid};
use crate::ordinary::{draft::types::{DraftClaim,SaveFrame},wire::{encode,Envelope,Frame,In,NativeObject,human_name}};
use std::{ffi::CString,io};

pub(in crate::ordinary) fn request(pool:PoolId,uid:u32,op:OperationId,file:FileId,revision:u64)->io::Result<[u8;32]>{
    if revision!=1{return Err(invalid("first manual commit requires exactly saved working revision 1"));}
    let mut b=b"ZXS/FIRST-MANUAL-FULL/REQUEST\0".to_vec();b.extend_from_slice(&pool.0);
    b.extend_from_slice(&uid.to_be_bytes());b.extend_from_slice(&op.0);b.extend_from_slice(&file.0);
    b.extend_from_slice(&revision.to_be_bytes());crate::hash::digest(&b)
}

#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Original{
    pub op:OperationId,pub uid:u32,pub file:FileId,pub history:HistoryId,pub volume:VolumeId,
    pub creation:[u8;32],pub save:OperationId,pub saved:[u8;32],pub content:DraftClaim,pub chain:[u8;32],
    pub branch:BranchId,pub node:NodeId,pub version:VersionId,pub storage:VersionStorageId,pub creation_name:String,
}
impl Original{
    pub fn select(op:OperationId,uid:u32,created:&Frame,save:&SaveFrame)->io::Result<Self>{
        if !save.original.matches_create(created){return Err(invalid("formal source is not the original saved FILE"));}
        Ok(Self{op,uid,file:created.create.file,history:created.create.history,volume:created.create.volume,creation:created.digest,
            save:save.original.op,saved:save.digest,content:save.original.claim,chain:save.material.chain,
            branch:BranchId::new()?,node:NodeId::new()?,version:VersionId::new()?,storage:VersionStorageId::fresh()?,
            creation_name:created.create.name.clone()})
    }
    pub fn matches(&self,c:&Frame,w:&SaveFrame)->bool{
        self.file==c.create.file&&self.history==c.create.history&&self.volume==c.create.volume&&self.creation==c.digest&&
            self.creation_name==c.create.name&&w.original.matches_create(c)&&self.save==w.original.op&&self.saved==w.digest&&
            self.content==w.original.claim&&self.chain==w.material.chain
    }
    pub fn body_name(&self)->CString{CString::new(format!("{}-{}",self.storage,self.creation_name)).expect("checked original FILE birth name")}
    pub fn node_name(&self)->CString{CString::new(format!("node-{}",self.node)).expect("fixed native node name")}
    pub fn put(&self,b:&mut Vec<u8>){
        b.extend_from_slice(&self.op.0);b.extend_from_slice(&self.uid.to_be_bytes());
        for v in [&self.file.0,&self.history.0,&self.volume.0]{b.extend_from_slice(v);}
        b.extend_from_slice(&self.creation);b.extend_from_slice(&self.save.0);b.extend_from_slice(&self.saved);
        b.extend_from_slice(&self.content.bytes.to_be_bytes());b.extend_from_slice(&self.content.sha256);b.extend_from_slice(&self.chain);
        for v in [&self.branch.0,&self.node.0,&self.version.0]{b.extend_from_slice(v);}
        b.extend_from_slice(&self.storage.0);b.push(self.creation_name.len() as u8);b.extend_from_slice(self.creation_name.as_bytes());
    }
    pub fn get(r:&mut In<'_>)->io::Result<Self>{
        let op=OperationId(r.id()?);let uid=r.u32()?;let file=FileId(r.id()?);let history=HistoryId(r.id()?);
        let volume=VolumeId(r.id()?);let creation=r.fixed()?;let save=OperationId(r.id()?);let saved=r.fixed()?;
        let content=DraftClaim::new(r.u64()?,r.fixed()?)?;let chain=r.fixed()?;
        let branch=BranchId(r.id()?);let node=NodeId(r.id()?);let version=VersionId(r.id()?);
        let storage=VersionStorageId::checked(r.fixed()?)?;let creation_name=human_name(r.short_buf()?)?.to_owned();
        Ok(Self{op,uid,file,history,volume,creation,save,saved,content,chain,branch,node,version,storage,creation_name})
    }
    /// Local FILE history has stable identities and creation-time facts only.
    /// Neither its current volume, physical inode, path ownership, nor a hash
    /// of a yet-unpublished SYSTEM frame is used as part of this node.
    pub fn node_header(&self)->Vec<u8>{
        let mut b=b"ZXS/FILE/FORMAL/FIRST-MAIN-FULL\0".to_vec();
        for id in [&self.file.0,&self.history.0,&self.op.0,&self.branch.0,&self.node.0,&self.version.0,&self.save.0]{b.extend_from_slice(id);}
        b.extend_from_slice(&1u64.to_be_bytes()); // exact working revision
        b.extend_from_slice(b"\x04main\x00"); // branch's initial name, zero parents
        b.extend_from_slice(&self.storage.0);b.push(self.creation_name.len() as u8);b.extend_from_slice(self.creation_name.as_bytes());
        b.extend_from_slice(&self.content.bytes.to_be_bytes());b.extend_from_slice(&self.content.sha256);b.extend_from_slice(&self.chain);b
    }
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Materials{pub node:NativeObject,pub raw:NativeObject,pub index_digest:[u8;32]}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Commit{
    pub seq:u64,pub previous:[u8;32],pub request:[u8;32],pub digest:[u8;32],pub bytes:Vec<u8>,
    pub original:Original,pub allocation:[u8;32],pub materials:Materials,
}
impl Commit{
    pub fn new(pool:PoolId,seq:u64,previous:[u8;32],original:Original,allocation:[u8;32],materials:Materials)->io::Result<Self>{
        let request=request(pool,original.uid,original.op,original.file,1)?;let mut body=Vec::new();
        original.put(&mut body);body.extend_from_slice(&allocation);materials.node.put(&mut body);materials.raw.put(&mut body);body.extend_from_slice(&materials.index_digest);
        let (bytes,digest)=encode(pool,seq,previous,3,request,&body)?;
        Ok(Self{seq,previous,request,digest,bytes,original,allocation,materials})
    }
    pub fn from_envelope(pool:PoolId,bytes:&[u8],e:Envelope<'_>)->io::Result<Self>{
        if e.kind!=3{return Err(invalid("not an original explicit formal commit"));}
        let mut r=In(e.body);let original=Original::get(&mut r)?;let allocation=r.fixed()?;
        let materials=Materials{node:NativeObject::get(&mut r)?,raw:NativeObject::get(&mut r)?,index_digest:r.fixed()?};r.end()?;
        if request(pool,original.uid,original.op,original.file,1)?!=e.request{return Err(invalid("different original explicit commit request"));}
        Ok(Self{seq:e.seq,previous:e.previous,request:e.request,digest:e.digest,bytes:bytes.to_vec(),original,allocation,materials})
    }
    pub fn receipt(&self)->FormalNode{
        let o=&self.original;
        FormalNode{file:o.file,history:o.history,branch:o.branch,node:o.node,version:o.version,storage:o.storage,created_by:o.op,
            created_from_save:o.save,working_revision:1,creation_name:o.creation_name.clone(),body:o.content,decision_sequence:self.seq}
    }
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub struct FormalNode{
    pub file:FileId,pub history:HistoryId,pub branch:BranchId,pub node:NodeId,pub version:VersionId,pub storage:VersionStorageId,
    pub created_by:OperationId,pub created_from_save:OperationId,pub working_revision:u64,pub creation_name:String,pub body:DraftClaim,pub decision_sequence:u64,
}
