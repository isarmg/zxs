//! First direct registered-root handoff: no rename of the logical FILE,
//! no new content or nodes, no new vote. This codec is not a stand-alone
//! admission or an I/O capability; replay must prove the prior custody cut.
use crate::{EntryId,FileId,HistoryId,OperationId,PoolId,VolumeId,fs::{self,invalid},ordinary::{
    delta::types::Delta,formal::types::Commit,wire::{self,Envelope,Frame,In,NativeObject}}};
use super::{purpose::{Call,Purpose},destination::{Method,Prepared},source::Verified};
use std::{collections::BTreeSet,io};

pub(in crate::ordinary) const KIND:u8=6;
pub(in crate::ordinary) fn request(pool:PoolId,uid:u32,op:OperationId,file:FileId,from:VolumeId,to:VolumeId,epoch:u64)->io::Result<[u8;32]>{
    Call::Move.request(pool,uid,op,file,from,to,epoch)
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Origin{
    pub op:OperationId,pub uid:u32,pub file:FileId,pub entry:EntryId,pub history:HistoryId,
    pub from:VolumeId,pub to:VolumeId,pub epoch:u64,pub name:String,pub ticket:OperationId,pub mode:Method,pub purpose:Purpose,
    pub src_root:NativeObject,pub dst_root:NativeObject,pub creation:[u8;32],pub parent:[u8;32],pub last:Option<[u8;32]>,
    pub source_bytes:u64,pub source_material:[u8;32],
}
fn take_hash(r:&mut In<'_>)->io::Result<Option<[u8;32]>>{
    match r.byte()?{0=>Ok(None),1=>Ok(Some(r.fixed()?)),_=>Err(invalid("unknown optional INCR in MOVE"))}
}
fn put_hash(out:&mut Vec<u8>,d:Option<[u8;32]>){
    out.push(u8::from(d.is_some()));if let Some(d)=d{out.extend_from_slice(&d);}
}
pub(super) fn isolation_name(ticket:OperationId)->std::ffi::CString{
    std::ffi::CString::new(format!(".zxs-move-out-{ticket}")).expect("fixed ASCII original MOVE ticket")
}
/// All actual (root,name) pairs that a transfer will later *have* to
/// reopen must pass the same full path/leaf/depth gate as child_directory.
/// This is only a bounded feasibility check, not a named write or authority.
pub(super) fn check_private_paths(old:&fs::HeldDir,new:&fs::HeldDir,ticket:OperationId)->io::Result<()>{
    old.checked_child_path(&isolation_name(ticket))?;
    for slot in 0..crate::ordinary::allocation::SLOTS{
        new.checked_child_path(&crate::ordinary::material::staging(ticket,slot))?;
    }Ok(())
}
impl Origin{
    pub(in crate::ordinary) fn effect(&self)->crate::ordinary::journal::credits::Effect{self.purpose.call().effect(self.file,self.op)}
    pub(super) fn request_digest(&self,pool:PoolId)->io::Result<[u8;32]>{
        self.purpose.call().request(pool,self.uid,self.op,self.file,self.from,self.to,self.epoch)
    }
    pub(super) fn select(op:OperationId,p:&Prepared<'_>,ticket:OperationId)->io::Result<Self>{
        let src=p.source();let (c,f,d)=src.originals();
        let o=Self{op,uid:fs::local_uid(),file:src.file(),entry:c.create.entry,history:src.history(),from:src.volume(),to:p.volume(),epoch:0,
            name:src.name().to_owned(),ticket,mode:p.method(),purpose:Purpose::Ordinary,src_root:src.source_root().clone(),dst_root:p.root().clone(),
            creation:c.digest,parent:f.digest,last:d.map(|d|d.digest),source_bytes:src.physical_bytes(),source_material:src.digest()?};
        o.require(src,p)?;Ok(o)
    }
    // The frozen roles are FILE, first node, first RAW, and only an already
    // committed INCR. Missing physical material cannot reduce the count.
    pub(super) fn role_count(&self)->usize{3+usize::from(self.last.is_some())}
    /// Only this MOVE issued the source-private isolation name on FROM.
    /// The internal spelling is not a business FILE or a scan pattern.
    pub(super) fn quarantine_name(&self)->std::ffi::CString{
        isolation_name(self.ticket)
    }
    pub(super) fn require(&self,src:&Verified,p:&Prepared<'_>)->io::Result<()>{
        if !std::ptr::eq(src,p.source())||src.pool()!=p.dest_pool()||self.src_root!=*src.source_root()||self.dst_root!=*p.root()||
            self.to!=p.volume()||self.mode!=p.method()||self.source_bytes!=src.physical_bytes()||self.source_material!=src.digest()?{
            return Err(invalid("different complete source or prepared target for the original MOVE"));
        }
        let(c,f,d)=src.originals();self.require_history(c,f,d)
    }
    pub(in crate::ordinary) fn require_history(&self,c:&Frame,f:&Commit,d:Option<&Delta>)->io::Result<()>{
        let local=wire::empty_material(c.create.file,c.create.history,c.create.op,&c.create.name).len() as u64;
        let node=crate::ordinary::formal::physical::index_size(&f.original)?;
        let child=d.map(|v|crate::ordinary::delta::physical::packed_size(&v.original)).transpose()?.unwrap_or(0);
        let bytes=local.checked_add(node).and_then(|v|v.checked_add(f.original.content.bytes)).and_then(|v|v.checked_add(child));
        if bytes!=Some(self.source_bytes)||self.epoch==u64::MAX||self.from==self.to||self.op==self.ticket||self.src_root==self.dst_root||
            self.uid!=c.create.uid||self.file!=c.create.file||self.history!=c.create.history||self.entry!=c.create.entry||
            (self.purpose==Purpose::Ordinary&&(self.from!=c.create.volume||self.epoch!=0))||self.name!=c.create.name||self.creation!=c.digest||self.parent!=f.digest||
            f.original.file!=self.file||f.original.creation!=c.digest||self.last!=d.map(|x|x.digest)||
            d.is_some_and(|x|x.original.file!=self.file||x.original.creation!=c.digest||x.original.parent_decision!=f.digest)||
            self.source_bytes==0{
            return Err(invalid("not the complete original FILE at its declared first custody cut"));
        }
        Ok(())
    }
    pub(in crate::ordinary) fn put(&self,b:&mut Vec<u8>)->io::Result<()>{
        wire::human_name(self.name.as_bytes())?;
        self.purpose.check_transfer(self.mode,&self.src_root,&self.dst_root)?;
        let start=b.len();
        for id in [&self.op.0,&self.file.0,&self.entry.0,&self.history.0,&self.from.0,&self.to.0,&self.ticket.0]{b.extend_from_slice(id);}
        b.extend_from_slice(&self.uid.to_be_bytes());b.extend_from_slice(&self.epoch.to_be_bytes());
        b.push(self.purpose.tag(self.mode)?);b.push(self.name.len() as u8);b.extend_from_slice(self.name.as_bytes());
        self.src_root.put(b);self.dst_root.put(b);b.extend_from_slice(&self.creation);b.extend_from_slice(&self.parent);put_hash(b,self.last);
        b.extend_from_slice(&self.source_bytes.to_be_bytes());b.extend_from_slice(&self.source_material);self.purpose.put(b);
        if b.len()-start>MAX_ORIGIN{return Err(invalid("custody origin exceeds the certified restore bound"));}Ok(())
    }
    pub(in crate::ordinary) fn get(r:&mut In<'_>)->io::Result<Self>{
        let op=OperationId(r.id()?);let file=FileId(r.id()?);let entry=EntryId(r.id()?);let history=HistoryId(r.id()?);
        let from=VolumeId(r.id()?);let to=VolumeId(r.id()?);let ticket=OperationId(r.id()?);let uid=r.u32()?;
        let epoch=r.u64()?;let tag=r.byte()?;let mode=Purpose::method(tag)?;
        let name=wire::human_name(r.short_buf()?)?.to_owned();
        let src_root=NativeObject::get(r)?;let dst_root=NativeObject::get(r)?;let creation=r.fixed()?;let parent=r.fixed()?;
        let last=take_hash(r)?;let source_bytes=r.u64()?;let source_material=r.fixed()?;
        let purpose=Purpose::get(tag,r)?;purpose.check_transfer(mode,&src_root,&dst_root)?;
        if op==ticket||from==to||epoch==u64::MAX||purpose==Purpose::Ordinary&&epoch!=0||source_bytes==0||src_root==dst_root{
            return Err(invalid("inconsistent original roots, epoch or method"));
        }
        Ok(Self{op,file,entry,history,from,to,ticket,uid,epoch,mode,purpose,name,src_root,dst_root,creation,parent,last,source_bytes,source_material})
    }
}
/// Physical custody alone, never replacement history. Stable version/node
/// references and their byte commitments remain in their original decisions.
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Sites{
    pub directory:NativeObject,pub local:NativeObject,pub node:NativeObject,pub raw:NativeObject,pub incr:Option<NativeObject>,
}
impl Sites{
    pub(in crate::ordinary) fn birthplace(c:&Frame,f:&Commit,d:Option<&Delta>)->Self{
        Self{directory:c.create.directory.clone(),local:c.create.local.clone(),node:f.materials.node.clone(),raw:f.materials.raw.clone(),
            incr:d.map(|x|x.material.object.clone())}
    }
    pub(super) fn at(&self,k:super::source::Leaf)->Option<&NativeObject>{
        use super::source::Leaf;match k{Leaf::File=>Some(&self.local),Leaf::FullNode=>Some(&self.node),Leaf::FullRaw=>Some(&self.raw),Leaf::Incr=>self.incr.as_ref()}
    }
    pub(in crate::ordinary) fn each(&self)->impl Iterator<Item=&NativeObject>{
        [&self.directory,&self.local,&self.node,&self.raw].into_iter().chain(self.incr.iter())
    }
    pub(in crate::ordinary) fn check(&self,root:&NativeObject)->io::Result<()>{
        let mut births=BTreeSet::new();let mut entities=BTreeSet::new();births.insert(root.birth.clone());entities.insert((root.entity.device,root.entity.inode));
        for x in self.each(){
            if x.entity.device!=root.entity.device||x.birth.filesystem()!=root.birth.filesystem()||
                !births.insert(x.birth.clone())||!entities.insert((x.entity.device,x.entity.inode)){
                return Err(invalid("current physical MOVE roles alias or are not on their registered root filesystem"));
            }
        }Ok(())
    }
    pub(in crate::ordinary) fn put(&self,b:&mut Vec<u8>){
        for n in [&self.directory,&self.local,&self.node,&self.raw]{n.put(b);}
        b.push(u8::from(self.incr.is_some()));if let Some(x)=&self.incr{x.put(b);}
    }
    pub(in crate::ordinary) fn get(r:&mut In<'_>)->io::Result<Self>{
        let directory=NativeObject::get(r)?;let local=NativeObject::get(r)?;let node=NativeObject::get(r)?;let raw=NativeObject::get(r)?;
        let incr=match r.byte()?{0=>None,1=>Some(NativeObject::get(r)?),_=>return Err(invalid("invalid incremental custody role"))};
        Ok(Self{directory,local,node,raw,incr})
    }
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Moved{pub seq:u64,pub previous:[u8;32],pub request:[u8;32],pub digest:[u8;32],pub bytes:Vec<u8>,
    pub original:Origin,pub allocation:[u8;32],pub stage:Option<u8>,pub target:Sites}
// Canonical maxima. The actual maximal encoder is also exercised in the
// source fixture. Omitting an optional INCR or a shorter name only shrinks it.
const NATIVE:usize=16+1+fs::MAX_BIRTH;
pub(super) const MAX_ORIGIN:usize=7*16+4+8+1+1+wire::MAX_HUMAN+2*NATIVE+2*32+1+32+8+32+super::purpose::MAX_EXTRA;
const MAX_CUSTODY_FRAME:usize=wire::ENVELOPE_BYTES+MAX_ORIGIN+32+1+(5*NATIVE+1);
const _:()=assert!(MAX_CUSTODY_FRAME<=wire::MAX_FRAME);
impl Moved{
    pub(in crate::ordinary) fn require_history(&self,c:&Frame,f:&Commit,d:Option<&Delta>)->io::Result<()>{
        if self.original.purpose!=Purpose::Ordinary{return Err(invalid("cannot use FILE birthplace as a later custody source"));}
        let before=crate::ordinary::placement::Placement::from_prefix(c,None)?;
        self.require_at(before,f,d).map(|_|())
    }
    pub(in crate::ordinary) fn require_at(&self,before:crate::ordinary::placement::Placement<'_>,f:&Commit,d:Option<&Delta>)->io::Result<Sites>{
        let o=&self.original;before.require_next(o)?;o.require_history(before.birth(),f,d)?;
        let previous=before.sites(f,d)?;previous.check(&o.src_root)?;self.target.check(&o.dst_root)?;
        if self.target.incr.is_some()!=d.is_some(){return Err(invalid("a MOVE cannot lose/add a retained version"));}
        match o.mode{
            Method::Rename if self.stage.is_none()&&self.target==previous=>Ok(previous),
            Method::Transfer if self.stage.is_some_and(|n|n<crate::ordinary::allocation::SLOTS)&&
                self.target.each().all(|x|!previous.each().any(|v|v.birth==x.birth||v.entity==x.entity))=>Ok(previous),
            _=>Err(invalid("wrong rename/transfer native material or alias with original FILE")),
        }
    }
    fn target_shape(o:&Origin,stage:Option<u8>,target:&Sites)->io::Result<()>{
        if target.incr.is_some()!=o.last.is_some()||match o.mode{
            Method::Rename=>stage.is_some(),Method::Transfer=>!stage.is_some_and(|n|n<crate::ordinary::allocation::SLOTS),
        }{return Err(invalid("wrong MOVE role set, mode or stage slot"));}
        target.check(&o.dst_root)
    }
    pub(super) fn seal(pool:PoolId,seq:u64,previous:[u8;32],original:Origin,allocation:[u8;32],stage:Option<u8>,target:Sites)->io::Result<Self>{
        if seq==0{return Err(invalid("zero MOVE sequence"));}Self::target_shape(&original,stage,&target)?;
        let req=original.request_digest(pool)?;
        let mut b=Vec::new();original.put(&mut b)?;b.extend_from_slice(&allocation);
        b.push(stage.map_or(0,|s|s+1));target.put(&mut b);
        let(bytes,digest)=wire::encode(pool,seq,previous,KIND,req,&b)?;
        Ok(Self{seq,previous,request:req,digest,bytes,original,allocation,stage,target})
    }
    pub(in crate::ordinary) fn read(pool:PoolId,bytes:&[u8],e:Envelope<'_>)->io::Result<Self>{
        if e.kind!=KIND{return Err(invalid("not a native physical MOVE decision"));}
        let mut r=In(e.body);let original=Origin::get(&mut r)?;let allocation=r.fixed()?;
        let window=r.byte()?;let stage=match window{0=>None,n if n<=crate::ordinary::allocation::SLOTS=>Some(n-1),_=>return Err(invalid("invalid original MOVE stage slot"))};
        let target=Sites::get(&mut r)?;r.end()?;
        if original.request_digest(pool)?!=e.request{
            return Err(invalid("wrong original MOVE principal, FILE, source, destination or generation"));
        }
        Self::target_shape(&original,stage,&target)?;
        Ok(Self{seq:e.seq,previous:e.previous,request:e.request,digest:e.digest,bytes:bytes.to_vec(),original,allocation,stage,target})
    }
}

#[cfg(test)]
#[path="../../../tests/unit/recycle_codec.rs"]
mod purpose_tests;
