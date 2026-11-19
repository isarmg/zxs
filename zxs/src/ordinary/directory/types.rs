use crate::{EntryId,OperationId,PoolId,VolumeId,identity::DirectoryId,fs::{self,invalid},
    pool::{JournalRegistration,lease::FileVolume}};
use crate::ordinary::{allocation::SLOTS,wire::{self,In,NativeObject}};
use std::io;

pub(in crate::ordinary) const KIND:u8=7;
/// An address, never a live grant. A child's own birth names the same volume;
/// no spelling or missing tag turns a child selection into the volume root.
#[derive(Clone,Copy,Debug,Eq,PartialEq,Ord,PartialOrd)]
pub enum Parent{Root(VolumeId),Child(VolumeId,DirectoryId)}
impl Parent{
    pub(in crate::ordinary) fn volume(self)->VolumeId{match self{Self::Root(v)|Self::Child(v,_)=>v}}
    pub(in crate::ordinary) fn put(self,b:&mut Vec<u8>){
        b.extend_from_slice(&self.volume().0);
        match self{Self::Root(_)=>b.push(0),Self::Child(_,id)=>{b.push(1);b.extend_from_slice(&id.0);}}
    }
    fn get(r:&mut In<'_>)->io::Result<Self>{
        let v=VolumeId(r.id()?);match r.byte()?{0=>Ok(Self::Root(v)),1=>Ok(Self::Child(v,DirectoryId(r.id()?))),
            _=>Err(invalid("unknown ordinary parent address"))}
    }
}

/// For a root, anchor is the original pool registration digest. For a child
/// it is that directory's original confirmed decision, not its latest listing.
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Basis{pub address:Parent,pub anchor:[u8;32],pub object:NativeObject}
impl Basis{
    pub(in crate::ordinary) fn registered(v:&FileVolume<'_>)->io::Result<Self>{
        v.require_current()?;let r=v.pool.journal_registration();let obj=NativeObject{entity:v.root.entity,birth:fs::Birth::read(&v.root.file)?};
        if !r.file(v.id(),fs::local_uid(),obj.entity,&obj.birth,true){return Err(invalid("different ordinary root at directory selection"));}
        v.require_current()?;Ok(Self{address:Parent::Root(v.id()),anchor:r.basis().2,object:obj})
    }
    /// This is a relation, not physical installation. The caller must obtain
    /// the parent only from its own already-admitted prefix, then open the
    /// exact parent lineage when using it for a new operation.
    pub(in crate::ordinary) fn require(&self,r:&JournalRegistration<'_>,uid:u32,previous:Option<&Created>)->io::Result<()>{
        if previous.is_some_and(|d|d.pool!=r.basis().0){return Err(invalid("directory parent frame was decoded against another SYSTEM"));}
        let obj=&self.object;let v=self.address.volume();
        let linked=match(self.address,previous){
            (Parent::Root(_),None)=>self.anchor==r.basis().2&&r.file(v,uid,obj.entity,&obj.birth,true),
            (Parent::Child(_,id),Some(d))=>d.origin.id==id&&d.origin.parent.address.volume()==v&&d.origin.uid==uid
                &&self.anchor==d.digest&&obj==&d.object&&r.directory_child(v,uid,obj.entity,&obj.birth),
            _=>false,
        };
        if !linked{return Err(invalid("parent is not the selected original root or directory birth"));}Ok(())
    }
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Origin{
    pub op:OperationId,pub uid:u32,pub parent:Basis,pub name:String,pub entry:EntryId,pub id:DirectoryId,pub ticket:OperationId,
}
pub(in crate::ordinary) fn request(pool:PoolId,op:OperationId,uid:u32,parent:Parent,name:&str)->io::Result<[u8;32]>{
    wire::human_name(name.as_bytes())?;
    let mut b=b"ZXS/ORDINARY-DIRECTORY-CREATE/1\0".to_vec();
    b.extend_from_slice(&pool.0);b.extend_from_slice(&op.0);b.extend_from_slice(&uid.to_be_bytes());
    parent.put(&mut b);b.push(name.len() as u8);b.extend_from_slice(name.as_bytes());crate::hash::digest(&b)
}
impl Origin{
    fn shape(&self)->io::Result<()>{
        wire::human_name(self.name.as_bytes())?;
        if self.op==self.ticket||matches!(self.parent.address,Parent::Child(_,id) if id==self.id){
            return Err(invalid("repeated operation ticket or directory is its own parent"));}Ok(())
    }
    pub(in crate::ordinary) fn put(&self,b:&mut Vec<u8>)->io::Result<()>{
        self.shape()?;b.extend_from_slice(&self.op.0);b.extend_from_slice(&self.uid.to_be_bytes());
        self.parent.address.put(b);b.extend_from_slice(&self.parent.anchor);self.parent.object.put(b);
        for id in [&self.entry.0,&self.id.0,&self.ticket.0]{b.extend_from_slice(id);}
        b.push(self.name.len() as u8);b.extend_from_slice(self.name.as_bytes());Ok(())
    }
    pub(in crate::ordinary) fn get(r:&mut In<'_>)->io::Result<Self>{
        let op=OperationId(r.id()?);let uid=r.u32()?;let address=Parent::get(r)?;let anchor=r.fixed()?;
        let object=NativeObject::get(r)?;let parent=Basis{address,anchor,object};
        let entry=EntryId(r.id()?);let id=DirectoryId(r.id()?);let ticket=OperationId(r.id()?);
        let name=wire::human_name(r.short_buf()?)?.to_owned();let o=Self{op,uid,parent,name,entry,id,ticket};o.shape()?;Ok(o)
    }
    pub(in crate::ordinary) fn check_child(&self,slot:u8,n:&NativeObject)->io::Result<()>{
        let p=&self.parent.object;
        if slot>=SLOTS||n.entity==p.entity||n.birth==p.birth||n.entity.device!=p.entity.device||n.birth.filesystem()!=p.birth.filesystem(){
            return Err(invalid("not a distinct child on the parent filesystem at an issued private slot"));}Ok(())
    }
}
const NATIVE:usize=16+1+fs::MAX_BIRTH;
pub(in crate::ordinary) const MAX_ORIGIN:usize=16+4+16+1+16+32+NATIVE+3*16+1+wire::MAX_HUMAN;
const MAX_BIRTH_FRAME:usize=wire::ENVELOPE_BYTES+MAX_ORIGIN+32+1+NATIVE;
const _:()=assert!(MAX_BIRTH_FRAME<=wire::MAX_FRAME);
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Created{
    pool:PoolId,pub seq:u64,pub previous:[u8;32],pub request:[u8;32],pub digest:[u8;32],pub bytes:Vec<u8>,
    pub origin:Origin,pub allocation:[u8;32],pub slot:u8,pub object:NativeObject,
}
impl Created{
    pub(super) fn belongs_to(&self,pool:PoolId)->bool{self.pool==pool}
    pub(in crate::ordinary) fn new(pool:PoolId,seq:u64,previous:[u8;32],origin:Origin,allocation:[u8;32],slot:u8,object:NativeObject)->io::Result<Self>{
        if seq==0{return Err(invalid("zero ordinary directory decision sequence"));}
        let claim=request(pool,origin.op,origin.uid,origin.parent.address,&origin.name)?;
        origin.check_child(slot,&object)?;
        let mut body=Vec::new();origin.put(&mut body)?;body.extend_from_slice(&allocation);body.push(slot);object.put(&mut body);
        let(bytes,_)=wire::encode(pool,seq,previous,KIND,claim,&body)?;
        Self::read(pool,&bytes)
    }
    pub(in crate::ordinary) fn read(pool:PoolId,bytes:&[u8])->io::Result<Self>{
        if bytes.len()>MAX_BIRTH_FRAME{return Err(invalid("oversize ordinary directory birth"));}
        let e=wire::envelope(pool,bytes)?;if e.kind!=KIND{return Err(invalid("not an ordinary directory birth"));}
        let mut r=In(e.body);let origin=Origin::get(&mut r)?;let allocation=r.fixed()?;let slot=r.byte()?;
        let object=NativeObject::get(&mut r)?;r.end()?;origin.check_child(slot,&object)?;
        if request(pool,origin.op,origin.uid,origin.parent.address,&origin.name)?!=e.request{
            return Err(invalid("different original mkdir request"));}
        Ok(Self{pool,seq:e.seq,previous:e.previous,request:e.request,digest:e.digest,bytes:bytes.to_vec(),origin,allocation,slot,object})
    }
    /// Verify against the caller's exact original parent *decision*. This
    /// does not certify installation, absence of a name, or permit new I/O.
    pub(in crate::ordinary) fn relation(&self,reg:&JournalRegistration<'_>,earlier:Option<&Self>)->io::Result<()>{
        let pool=reg.basis().0;
        if self.pool!=pool{
            return Err(invalid("directory and its original parent must belong to the same registered SYSTEM"));}
        let o=&self.origin;o.parent.require(reg,o.uid,earlier)?;
        if earlier.is_some_and(|p|p.seq>=self.seq)||!reg.directory_child(o.parent.address.volume(),o.uid,self.object.entity,&self.object.birth){
            return Err(invalid("not an original ordinary child preceded by its selected parent"));}Ok(())
    }
}

#[cfg(test)]
#[path="../../../tests/unit/directory_birth.rs"]
mod tests;
