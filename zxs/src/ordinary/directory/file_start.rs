//! Prospective FILE creation under one installed directory. This observes a
//! new operation against the live SYSTEM and prepares an anonymous SYSTEM
//! operand only. There is deliberately no public, cold or naming route here.
use super::{inventory,path::HeldParent,types::{Basis,Parent}};
use crate::{DirectoryId,EntryId,FileId,HistoryId,OperationId,fs::{self,invalid},
    ordinary::{allocation::Terms,file_parent::BirthParent,material,wire::NativeObject},pool::lease::{Access,FileVolume,LocalPool}};
use std::{fs::File,io,os::unix::fs::MetadataExt};

struct Claim{
    op:OperationId,uid:u32,parent:BirthParent,name:String,entry:EntryId,file:FileId,history:HistoryId,ticket:OperationId,
    seq:u64,previous:[u8;32],root:NativeObject,
}
impl Claim{
    fn terms(&self,p:&LocalPool,v:crate::VolumeId)->Terms<'_>{
        Terms{pool:p.id(),op:self.op,uid:self.uid,volume:v,parent:&self.parent,name:&self.name,seq:self.seq,
            previous:self.previous,entry:self.entry,file:self.file,history:self.history,attempt:self.ticket,root:&self.root}
    }
    fn require(&self,h:&HeldParent<'_,'_>)->io::Result<()>{
        let v=h.volume();let p=v.pool;if !matches!(p.access,Access::Write)||fs::local_uid()!=self.uid{
            return Err(invalid("child FILE birth requires the original local exclusive writer"));}
        let(_,s)=p.snapshot()?;let t=self.terms(p,v.id());s.prospective_child_file(&t,&p.journal_registration())?;
        let basis=h.basis();
        if !matches!(&self.parent,BirthParent::Child{directory,decision,object} if basis.address==Parent::Child(v.id(),*directory)&&basis.anchor==*decision&&basis.object==*object)
            ||Basis::registered(v)?.object!=self.root{return Err(invalid("not the selected original child parent and registered root"));}
        // Census checks the held lineage and all authorized direct names. An
        // occupied name can veto a proposal, never confer parent authority.
        let all:Vec<_>=inventory::census(h,&s)?.into_iter().collect();
        let leaf=crate::ordinary::storage_name::file_leaf(&self.name,self.file)?;material::check_leaf(h.directory(),&all,&leaf,false)?;
        material::held_residence(v,h.directory(),&self.name,self.file,self.ticket)?;
        for i in 0..crate::ordinary::allocation::SLOTS{material::check_leaf(h.directory(),&all,&material::staging(self.ticket,i),false)?;}
        material::check_leaf(h.directory(),&all,&crate::ordinary::custody::transfer::isolation_name(self.ticket),false)?;
        material::room_for_ticket(&all,leaf.to_str().map_err(|_|invalid("invalid physical container name"))?,&material::staging(self.ticket,0))
    }
}
/// The FILE ids and bytes are kept with THIS physical anonymous issuer. No
/// externally supplied frame, directory, inode or private ticket is accepted.
pub(in crate::ordinary) struct Unissued<'v,'p>{
    parent:HeldParent<'v,'p>,claim:Claim,inode:File,issuer:NativeObject,bytes:Vec<u8>,digest:[u8;32],
}
impl<'v,'p> Unissued<'v,'p>{
    pub(in crate::ordinary) fn select(v:&'v FileVolume<'p>,parent:DirectoryId,op:OperationId,name:&str)->io::Result<Self>{
        let p=v.pool;if !matches!(p.access,Access::Write){return Err(invalid("not a child FILE writer"));}
        p.require_business_op(op)?;super::super::wire::human_name(name.as_bytes())?;
        let address=Parent::Child(v.id(),parent);let parent=HeldParent::open(v,address)?;
        let(_,s)=p.snapshot()?;let uid=fs::local_uid();let scope=s.file_parent_at(address,uid,&p.journal_registration())?;
        let(n,previous)=s.frontier();let claim=Claim{op,uid,parent:scope,name:name.to_owned(),entry:EntryId::new()?,file:FileId::new()?,
            history:HistoryId::new()?,ticket:OperationId::new()?,seq:n.checked_add(1).ok_or_else(||invalid("FILE sequence overflow"))?,
            previous,root:Basis::registered(v)?.object};drop(s);
        claim.require(&parent)?;
        let(inode,entity)=p.system.anonymous_record()?;let issuer=NativeObject{entity,birth:fs::Birth::read(&inode)?};
        let(bytes,digest)=claim.terms(p,v.id()).image(&issuer)?;
        // No name, xattr, reservation, stage or persisted business state.
        let result=Self{parent,claim,inode,issuer,bytes,digest};result.require()?;Ok(result)
    }
    pub(in crate::ordinary) fn require(&self)->io::Result<()>{
        self.claim.require(&self.parent)?;
        let v=self.parent.volume();let p=v.pool;let m=self.inode.metadata()?;
        if !m.is_file()||m.nlink()!=0||m.len()!=0||m.uid()!=self.claim.uid||m.mode()&0o7777!=0o600||
            m.dev()!=self.issuer.entity.device||m.ino()!=self.issuer.entity.inode||fs::Birth::read(&self.inode)?!=self.issuer.birth||
            !p.system.shares_file_mount(&self.inode)?{return Err(invalid("not the exact private anonymous SYSTEM proposal"));}
        let(bytes,digest)=self.claim.terms(p,v.id()).image(&self.issuer)?;
        if bytes!=self.bytes||digest!=self.digest{return Err(invalid("different original bytes for the child FILE proposal"));}Ok(())
    }
}

#[cfg(test)]
#[path="../../../tests/unit/child_file_start.rs"]
mod tests;
