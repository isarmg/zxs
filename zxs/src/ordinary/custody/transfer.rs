//! First direct registered-root handoff: no rename of the logical FILE,
//! no new content or nodes, no new vote. This codec is not a stand-alone
//! admission or an I/O capability; replay must prove the prior custody cut.
use crate::{EntryId,FileId,HistoryId,OperationId,PoolId,VolumeId,fs::{self,invalid},ordinary::{
    delta::types::Delta,formal::types::Commit,journal::Graph,wire::{self,Envelope,Frame,In,NativeObject}}};
use super::{purpose::{Call,Purpose},destination::{Method,Prepared},source::Verified};
use std::{collections::BTreeSet,io};

mod children;
mod parents;
pub(in crate::ordinary) use parents::{Parents,Choice as ParentChoice,End as ParentEnd};
pub(in crate::ordinary) use children::ChildRoles;
pub(in crate::ordinary) type Children<'a>=ChildRoles<&'a Delta>;

pub(in crate::ordinary) const KIND:u8=6;
pub(in crate::ordinary) fn request(pool:PoolId,uid:u32,op:OperationId,file:FileId,from:VolumeId,to:VolumeId,epoch:u64)->io::Result<[u8;32]>{
    Call::Move.request(pool,uid,op,file,from,to,epoch)
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Origin{
    pub op:OperationId,pub uid:u32,pub file:FileId,pub entry:EntryId,pub history:HistoryId,
    pub from:VolumeId,pub to:VolumeId,pub epoch:u64,pub name:String,pub ticket:OperationId,pub mode:Method,pub purpose:Purpose,pub parents:Parents,
    pub src_root:NativeObject,pub dst_root:NativeObject,pub creation:[u8;32],pub parent:[u8;32],pub children:ChildRoles<[u8;32]>,
    pub source_bytes:u64,pub source_material:[u8;32],
}
pub(in crate::ordinary) fn isolation_name(ticket:OperationId)->std::ffi::CString{
    crate::ordinary::storage_name::TicketLeaf::isolation(ticket).expect("validated original transfer ticket").name()
}
/// All actual (root,name) pairs that a transfer will later *have* to
/// reopen must pass the same full path/leaf/depth gate as child_directory.
/// This is only a bounded feasibility check, not a named write or authority.
pub(super) fn check_private_paths(old:&fs::HeldDir,new:&fs::HeldDir,ticket:OperationId)->io::Result<()>{
    let a=fs::NameBudget::held(old)?;let b=fs::NameBudget::held(new)?;
    a.child(&isolation_name(ticket))?;
    for slot in 0..crate::ordinary::allocation::SLOTS{
        b.child(&crate::ordinary::material::staging(ticket,slot))?;
    }Ok(())
}
impl Origin{
    pub(in crate::ordinary) fn parent_choice(&self)->ParentChoice{self.parents.choice(self.from,self.to)}
    pub(in crate::ordinary) fn ends(&self)->io::Result<(ParentEnd,ParentEnd)>{self.parent_choice().ends(self.purpose.call(),self.from,self.to)}
    pub(super) fn check_ordinary(&self,h:&crate::ordinary::OrdinaryParentLease<'_,'_>,source:bool)->io::Result<()>{
        let v=h.volume();let(a,b)=self.ends()?;let(end,root)=if source{(a,&self.src_root)}else{(b,&self.dst_root)};
        if end.ordinary()!=Some(h.address())||!self.parents.identifies(source,v.id(),h.basis()){
            return Err(invalid("not the original ordinary directory at this exact custody endpoint"));}
        super::placed::root(v,root)?;h.require()
    }
    pub(super) fn ordinary_at<'p>(&self,p:&'p crate::pool::lease::LocalPool,s:&crate::ordinary::journal::Journal,source:bool)
        ->io::Result<crate::ordinary::OrdinaryParentLease<'p,'p>>{
        let(a,b)=self.ends()?;let address=(if source{a}else{b}).ordinary().ok_or_else(||invalid("recycle is not an ordinary directory"))?;
        let h=crate::ordinary::OrdinaryParentLease::owned(p,address,s)?;
        self.check_ordinary(&h,source)?;Ok(h)
    }
    pub(in crate::ordinary) fn effect(&self)->crate::ordinary::journal::credits::Effect{self.purpose.call().effect(self.file,self.op)}
    pub(super) fn request_digest(&self,pool:PoolId)->io::Result<[u8;32]>{
        self.parent_choice().request(self.purpose.call(),pool,self.uid,self.op,self.file,self.from,self.to,self.epoch)
    }
    pub(super) fn select(op:OperationId,p:&Prepared<'_>,ticket:OperationId)->io::Result<Self>{
        let src=p.source();let (c,f,d)=src.originals();
        let o=Self{op,uid:fs::local_uid(),file:src.file(),entry:c.create.entry,history:src.history(),from:src.volume(),to:p.volume(),epoch:0,
            name:src.name().to_owned(),ticket,mode:p.method(),purpose:Purpose::Ordinary,parents:p.parents().clone(),src_root:src.source_root().clone(),dst_root:p.root().clone(),
            creation:c.digest,parent:f.digest,children:d.map(|d|d.digest),source_bytes:src.physical_bytes(),source_material:src.digest()?};
        o.require(src,p)?;Ok(o)
    }
    // The frozen roles are FILE, first node, first RAW, and only an already
    // committed child for each native decision. Missing material cannot reduce the count.
    pub(super) fn role_count(&self)->usize{3+self.children.len()}
    /// Only this MOVE issued the source-private isolation name on FROM.
    /// The internal spelling is not a business FILE or a scan pattern.
    pub(super) fn quarantine_name(&self)->std::ffi::CString{
        isolation_name(self.ticket)
    }
    pub(super) fn require(&self,src:&Verified,p:&Prepared<'_>)->io::Result<()>{
        if &self.parents!=p.parents()||!std::ptr::eq(src,p.source())||src.pool()!=p.dest_pool()||self.src_root!=*src.source_root()||self.dst_root!=*p.root()||
            self.to!=p.volume()||self.mode!=p.method()||self.source_bytes!=src.physical_bytes()||self.source_material!=src.digest()?{
            return Err(invalid("different complete source or prepared target for the original MOVE"));
        }
        let(c,f,d)=src.originals();self.require_history(c,f,d)
    }
    pub(in crate::ordinary) fn require_history(&self,c:&Frame,f:&Commit,d:Children<'_>)->io::Result<()>{
        self.require_graph(c,Graph::from_roles(c,f,d)?)
    }
    pub(in crate::ordinary) fn require_graph(&self,c:&Frame,g:Graph<'_>)->io::Result<()>{
        let f=g.full();let local=wire::empty_material(c.create.file,c.create.history,c.create.op,&c.create.name).len() as u64;
        let node=crate::ordinary::formal::physical::index_size(&f.original)?;
        let mut bytes=local.checked_add(node).and_then(|v|v.checked_add(f.original.content.bytes));
        for d in g.children().iter(){let n=crate::ordinary::delta::physical::packed_size(&d.original)?;bytes=bytes.and_then(|v|v.checked_add(n));}
        if bytes!=Some(self.source_bytes)||self.epoch==u64::MAX||self.from==self.to||self.op==self.ticket||self.src_root==self.dst_root||
            self.uid!=c.create.uid||self.file!=c.create.file||self.history!=c.create.history||self.entry!=c.create.entry||
            (self.purpose==Purpose::Ordinary&&(self.from!=c.create.volume||self.epoch!=0||self.ends()?.0.ordinary()!=Some(c.create.parent.address(c.create.volume))))||self.name!=c.create.name||self.creation!=c.digest||self.parent!=f.digest||
            f.original.file!=self.file||f.original.creation!=c.digest||self.children!=g.hashes()||
            self.source_bytes==0{
            return Err(invalid("not the complete original FILE at its declared first custody cut"));
        }
        Ok(())
    }
    fn child_hashes(&self)->io::Result<()>{
        if self.children.iter().any(|h|h==&self.parent||h==&self.creation)||
            self.children.second().is_some_and(|v|Some(v)==self.children.first()){
            return Err(invalid("repeated original custody history decision"));
        }Ok(())
    }
    pub(in crate::ordinary) fn put(&self,b:&mut Vec<u8>)->io::Result<()>{
        self.child_hashes()?;wire::human_name(self.name.as_bytes())?;
        self.purpose.check_transfer(self.mode,&self.src_root,&self.dst_root)?;
        self.parents.check(self.purpose.call(),self.from,self.to,&self.src_root,&self.dst_root)?;
        let start=b.len();
        for id in [&self.op.0,&self.file.0,&self.entry.0,&self.history.0,&self.from.0,&self.to.0,&self.ticket.0]{b.extend_from_slice(id);}
        b.extend_from_slice(&self.uid.to_be_bytes());b.extend_from_slice(&self.epoch.to_be_bytes());
        b.push(self.parents.tag(self.purpose.tag(self.mode)?));b.push(self.name.len() as u8);b.extend_from_slice(self.name.as_bytes());
        self.src_root.put(b);self.dst_root.put(b);b.extend_from_slice(&self.creation);b.extend_from_slice(&self.parent);self.children.put(b,|d,b|b.extend_from_slice(d));
        b.extend_from_slice(&self.source_bytes.to_be_bytes());b.extend_from_slice(&self.source_material);self.purpose.put(b);self.parents.put(b);
        if b.len()-start>MAX_ORIGIN{return Err(invalid("custody origin exceeds the certified restore bound"));}Ok(())
    }
    pub(in crate::ordinary) fn get(r:&mut In<'_>)->io::Result<Self>{
        let op=OperationId(r.id()?);let file=FileId(r.id()?);let entry=EntryId(r.id()?);let history=HistoryId(r.id()?);
        let from=VolumeId(r.id()?);let to=VolumeId(r.id()?);let ticket=OperationId(r.id()?);let uid=r.u32()?;
        let epoch=r.u64()?;let(tag,scoped)=Parents::split(r.byte()?)?;let mode=Purpose::method(tag)?;
        let name=wire::human_name(r.short_buf()?)?.to_owned();
        let src_root=NativeObject::get(r)?;let dst_root=NativeObject::get(r)?;let creation=r.fixed()?;let parent=r.fixed()?;
        let children=ChildRoles::get(r,|r|r.fixed())?;let source_bytes=r.u64()?;let source_material=r.fixed()?;
        let purpose=Purpose::get(tag,r)?;purpose.check_transfer(mode,&src_root,&dst_root)?;
        let parents=Parents::get(scoped,purpose.call(),r)?;parents.check(purpose.call(),from,to,&src_root,&dst_root)?;
        if op==ticket||from==to||epoch==u64::MAX||purpose==Purpose::Ordinary&&epoch!=0||source_bytes==0||src_root==dst_root{
            return Err(invalid("inconsistent original roots, epoch or method"));
        }
        let o=Self{op,file,entry,history,from,to,ticket,uid,epoch,mode,purpose,parents,name,src_root,dst_root,creation,parent,children,source_bytes,source_material};o.child_hashes()?;Ok(o)
    }
}
/// Physical custody alone, never replacement history. Stable version/node
/// references and their byte commitments remain in their original decisions.
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Sites{
    pub directory:NativeObject,pub local:NativeObject,pub node:NativeObject,pub raw:NativeObject,pub children:ChildRoles<NativeObject>,
}
impl Sites{
    pub(in crate::ordinary) fn birthplace(c:&Frame,f:&Commit,d:Children<'_>)->Self{
        Self{directory:c.create.directory.clone(),local:c.create.local.clone(),node:f.materials.node.clone(),raw:f.materials.raw.clone(),
            children:d.map(|x|x.material.object.clone())}
    }
    pub(super) fn at(&self,k:super::source::Leaf)->Option<&NativeObject>{
        use super::source::Leaf;match k{Leaf::File=>Some(&self.local),Leaf::FullNode=>Some(&self.node),Leaf::FullRaw=>Some(&self.raw),Leaf::Incr=>self.children.first(),Leaf::IncrNext=>self.children.second()}
    }
    pub(in crate::ordinary) fn each(&self)->impl Iterator<Item=&NativeObject>{
        [&self.directory,&self.local,&self.node,&self.raw].into_iter().chain(self.children.iter())
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
        self.children.put(b,|x,b|x.put(b));
    }
    pub(in crate::ordinary) fn get(r:&mut In<'_>)->io::Result<Self>{
        let directory=NativeObject::get(r)?;let local=NativeObject::get(r)?;let node=NativeObject::get(r)?;let raw=NativeObject::get(r)?;
        let children=ChildRoles::get(r,|r|NativeObject::get(r))?;
        Ok(Self{directory,local,node,raw,children})
    }
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Moved{pub seq:u64,pub previous:[u8;32],pub request:[u8;32],pub digest:[u8;32],pub bytes:Vec<u8>,
    pub original:Origin,pub allocation:[u8;32],pub stage:Option<u8>,pub target:Sites}
// Canonical maxima. The actual maximal encoder is also exercised in the
// source fixture. Omitting an optional INCR or a shorter name only shrinks it.
const NATIVE:usize=16+1+fs::MAX_BIRTH;
const COMMON_ORIGIN:usize=7*16+4+8+1+1+wire::MAX_HUMAN+2*NATIVE+2*32+1+2*32+8+32;
#[cfg(test)]
const ROOT_ORIGIN:usize=COMMON_ORIGIN+super::purpose::MAX_EXTRA;
const CHILD_ANCHOR:usize=1+16+32+NATIVE;
// Two physical child anchors for MOVE; DELETE has just one. RESTORE
// carries a historical ordinary address, not another live directory proof.
pub(super) const MAX_ORIGIN:usize=COMMON_ORIGIN+2*CHILD_ANCHOR;
const _:()=assert!(super::purpose::MAX_EXTRA+33+CHILD_ANCHOR<=2*CHILD_ANCHOR);
#[cfg(test)]
const ROOT_FRAME:usize=wire::ENVELOPE_BYTES+ROOT_ORIGIN+32+1+(6*NATIVE+1);
const MAX_CUSTODY_FRAME:usize=wire::ENVELOPE_BYTES+MAX_ORIGIN+32+1+(6*NATIVE+1);
const _:()=assert!(MAX_CUSTODY_FRAME<=wire::MAX_FRAME);
impl Moved{
    pub(in crate::ordinary) fn require_history(&self,c:&Frame,f:&Commit,d:Children<'_>)->io::Result<()>{
        if self.original.purpose!=Purpose::Ordinary{return Err(invalid("cannot use FILE birthplace as a later custody source"));}
        let before=crate::ordinary::placement::Placement::from_prefix(c,None)?;
        self.require_at(before,f,d).map(|_|())
    }
    pub(in crate::ordinary) fn require_at<'a>(&self,before:crate::ordinary::placement::Placement<'a>,f:&'a Commit,d:Children<'a>)->io::Result<Sites>{
        self.require_at_graph(before,Graph::from_roles(before.birth(),f,d)?)
    }
    pub(in crate::ordinary) fn require_at_graph<'a>(&self,before:crate::ordinary::placement::Placement<'a>,g:Graph<'a>)->io::Result<Sites>{
        let o=&self.original;before.require_next(o)?;o.require_graph(before.birth(),g)?;
        let previous=before.graph_sites(g)?;previous.check(&o.src_root)?;self.target.check(&o.dst_root)?;
        o.parents.check_material(&previous)?;o.parents.check_material(&self.target)?;
        if self.target.children.len()!=g.children().len(){return Err(invalid("a MOVE cannot lose/add a retained version"));}
        match o.mode{
            Method::Rename if self.stage.is_none()&&self.target==previous=>Ok(previous),
            Method::Transfer if self.stage.is_some_and(|n|n<crate::ordinary::allocation::SLOTS)&&
                self.target.each().all(|x|!previous.each().any(|v|v.birth==x.birth||v.entity==x.entity))=>Ok(previous),
            _=>Err(invalid("wrong rename/transfer native material or alias with original FILE")),
        }
    }
    fn target_shape(o:&Origin,stage:Option<u8>,target:&Sites)->io::Result<()>{
        if target.children.len()!=o.children.len()||match o.mode{
            Method::Rename=>stage.is_some(),Method::Transfer=>!stage.is_some_and(|n|n<crate::ordinary::allocation::SLOTS),
        }{return Err(invalid("wrong MOVE role set, mode or stage slot"));}
        o.parents.check_material(target)?;target.check(&o.dst_root)
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
