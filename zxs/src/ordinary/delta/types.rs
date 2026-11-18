//! A second, incremental formal node on the original branch, not a working
//! save or a second independent FILE. The completed SYSTEM decision is small:
//! it binds the whole immutable local INCR; the variable run map stays there.
use crate::{BranchId,FileId,HistoryId,NodeId,OperationId,PoolId,VersionId,VersionStorageId,VolumeId,fs::invalid};
use crate::ordinary::{draft::types::DraftClaim,formal::types::{FormalNode,FormalStorage},journal::cycle::Parent,
    interval::types::Saved,wire::{self,Envelope,In,NativeObject}};
use super::map::Map;
use crate::ordinary::placement::Placement;
use std::{ffi::CString,io};

pub(in crate::ordinary) fn request(pool:PoolId,uid:u32,op:OperationId,file:FileId,rev:u64)->io::Result<[u8;32]>{
    if rev<2{return Err(invalid("an explicit INCR requires an actual saved change after the formal parent"));}
    let mut b=b"ZXS/FORMAL/INCR/REQUEST\0".to_vec();b.extend_from_slice(&pool.0);b.extend_from_slice(&uid.to_be_bytes());
    b.extend_from_slice(&op.0);b.extend_from_slice(&file.0);b.extend_from_slice(&rev.to_be_bytes());crate::hash::digest(&b)
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Original{
    pub op:OperationId,pub uid:u32,pub file:FileId,pub history:HistoryId,pub volume:VolumeId,pub creation:[u8;32],
    pub branch:BranchId,pub parent:NodeId,pub parent_version:VersionId,pub parent_decision:[u8;32],pub parent_length:u64,
    pub working:OperationId,pub working_decision:[u8;32],pub revision:u64,pub logical:u64,pub changed:u64,pub rows:u32,pub map:[u8;32],
    pub node:NodeId,pub version:VersionId,pub storage:VersionStorageId,pub name:String,
}
impl Original{
    pub fn select(op:OperationId,uid:u32,place:Placement<'_>,p:Parent,w:&Saved,m:&Map)->io::Result<Self>{
        let c=place.birth();
        let o=Self{op,uid,file:c.create.file,history:c.create.history,volume:place.volume(),creation:c.digest,
            branch:p.branch,parent:p.node,parent_version:p.version,parent_decision:p.decision,parent_length:p.length,
            working:w.original.op,working_decision:w.digest,revision:w.original.revision(),logical:m.logical,changed:m.changed,rows:m.rows(),map:m.digest()?,
            node:NodeId::new()?,version:VersionId::new()?,storage:VersionStorageId::fresh()?,name:c.create.name.clone()};
        o.require(place,p,w,m)?;Ok(o)
    }
    pub fn require(&self,place:Placement<'_>,p:Parent,w:&Saved,m:&Map)->io::Result<()>{
        let c=place.birth();
        if place.retained().is_some()||!p.owns(w)||self.file!=c.create.file||self.history!=c.create.history||self.volume!=place.volume()||w.original.volume!=place.volume()||self.creation!=c.digest||
            self.name!=c.create.name||p.file!=self.file||p.creation!=c.digest||self.branch!=p.branch||
            self.parent!=p.node||self.parent_version!=p.version||self.parent_decision!=p.decision||self.parent_length!=p.length||
            w.original.file!=self.file||w.original.node!=self.parent||w.original.commit!=p.decision||self.working!=w.original.op||
            self.working_decision!=w.digest||self.revision!=w.original.revision()||self.logical!=w.original.result||
            self.logical!=m.logical||self.changed!=m.changed||self.rows!=m.rows()||self.map!=m.digest()?||
            self.node==self.parent||self.version==self.parent_version||self.storage==p.storage{
            return Err(invalid("formal INCR did not originate on this exact FILE, branch, parent and saved working cut"));
        }Ok(())
    }
    pub fn leaf(&self)->CString{CString::new(format!("{}-{}",self.storage,self.name)).expect("checked immutable version name")}
    pub fn put(&self,b:&mut Vec<u8>){
        for v in [&self.op.0,&self.file.0,&self.history.0,&self.volume.0]{b.extend_from_slice(v);}
        b.extend_from_slice(&self.uid.to_be_bytes());b.extend_from_slice(&self.creation);
        for v in [&self.branch.0,&self.parent.0,&self.parent_version.0]{b.extend_from_slice(v);}
        b.extend_from_slice(&self.parent_decision);b.extend_from_slice(&self.parent_length.to_be_bytes());
        b.extend_from_slice(&self.working.0);b.extend_from_slice(&self.working_decision);
        for v in [self.revision,self.logical,self.changed]{b.extend_from_slice(&v.to_be_bytes());}
        b.extend_from_slice(&self.rows.to_be_bytes());b.extend_from_slice(&self.map);
        b.extend_from_slice(&self.node.0);b.extend_from_slice(&self.version.0);b.extend_from_slice(&self.storage.0);
        b.push(self.name.len() as u8);b.extend_from_slice(self.name.as_bytes());
    }
    pub fn get(r:&mut In<'_>)->io::Result<Self>{
        let o=Self{op:OperationId(r.id()?),file:FileId(r.id()?),history:HistoryId(r.id()?),volume:VolumeId(r.id()?),
            uid:r.u32()?,creation:r.fixed()?,branch:BranchId(r.id()?),parent:NodeId(r.id()?),parent_version:VersionId(r.id()?),
            parent_decision:r.fixed()?,parent_length:r.u64()?,working:OperationId(r.id()?),working_decision:r.fixed()?,
            revision:r.u64()?,logical:r.u64()?,changed:r.u64()?,rows:r.u32()?,map:r.fixed()?,node:NodeId(r.id()?),
            version:VersionId(r.id()?),storage:VersionStorageId::checked(r.fixed()?)?,name:wire::human_name(r.short_buf()?)?.to_owned()};
        Map::checked_bytes(o.rows)?;
        if o.revision<2||o.changed>o.logical||o.logical>u64::MAX/8||o.parent_length>u64::MAX/8||
            (o.logical==0)!=(o.rows==0)||o.node==o.parent||o.version==o.parent_version{
            return Err(invalid("invalid formal incremental node shape"));
        }Ok(o)
    }
    pub fn prefix(&self,v:&Content)->Vec<u8>{
        let mut b=b"ZXS/FILE/FORMAL/INCR\0".to_vec();
        for id in [&self.file.0,&self.history.0,&self.op.0,&self.branch.0,&self.node.0,&self.version.0,
            &self.parent.0,&self.parent_version.0,&self.working.0]{b.extend_from_slice(id);}
        b.extend_from_slice(&self.storage.0);b.push(self.name.len() as u8);b.extend_from_slice(self.name.as_bytes());
        for n in [self.revision,self.parent_length,self.logical,self.changed]{b.extend_from_slice(&n.to_be_bytes());}
        b.extend_from_slice(&v.logical);b.extend_from_slice(&v.packed);b.extend_from_slice(&v.head);b
    }
    pub fn header(&self,m:&Map,v:&Content)->Vec<u8>{let mut b=self.prefix(v);m.put(&mut b);b}
}
#[derive(Clone,Debug,Default,Eq,PartialEq)]
pub(in crate::ordinary) struct Content{pub logical:[u8;32],pub packed:[u8;32],pub head:[u8;32]}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Material{pub object:NativeObject,pub content:Content,pub index:[u8;32]}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Delta{
    pub seq:u64,pub previous:[u8;32],pub request:[u8;32],pub digest:[u8;32],pub bytes:Vec<u8>,
    pub original:Original,pub allocation:[u8;32],pub material:Material,
}
impl Delta{
    pub fn new(pool:PoolId,seq:u64,previous:[u8;32],original:Original,allocation:[u8;32],material:Material)->io::Result<Self>{
        let req=request(pool,original.uid,original.op,original.file,original.revision)?;
        let mut b=Vec::new();original.put(&mut b);b.extend_from_slice(&allocation);material.object.put(&mut b);
        for v in [&material.content.logical,&material.content.packed,&material.content.head,&material.index]{b.extend_from_slice(v);}
        let (bytes,digest)=wire::encode(pool,seq,previous,5,req,&b)?;Ok(Self{seq,previous,request:req,digest,bytes,original,allocation,material})
    }
    pub fn from_envelope(pool:PoolId,bytes:&[u8],e:Envelope<'_>)->io::Result<Self>{
        if e.kind!=5{return Err(invalid("not an original formal INCR"));}
        let mut r=In(e.body);let original=Original::get(&mut r)?;let allocation=r.fixed()?;
        let material=Material{object:NativeObject::get(&mut r)?,content:Content{logical:r.fixed()?,packed:r.fixed()?,head:r.fixed()?},index:r.fixed()?};r.end()?;
        if request(pool,original.uid,original.op,original.file,original.revision)?!=e.request{return Err(invalid("different original formal INCR request"));}
        Ok(Self{seq:e.seq,previous:e.previous,request:e.request,digest:e.digest,bytes:bytes.to_vec(),original,allocation,material})
    }
    pub fn receipt(&self)->FormalNode{
        let o=&self.original;FormalNode{file:o.file,history:o.history,branch:o.branch,node:o.node,version:o.version,storage:o.storage,
            created_by:o.op,created_from_save:o.working,working_revision:o.revision,creation_name:o.name.clone(),
            body:DraftClaim{bytes:o.logical,sha256:self.material.content.logical},decision_sequence:self.seq,
            kind:FormalStorage::Incremental{parent:o.parent,parent_version:o.parent_version,changed:DraftClaim{bytes:o.changed,sha256:self.material.content.packed}}}
    }
}
