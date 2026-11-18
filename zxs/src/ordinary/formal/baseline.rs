//! The actual parent material of one confirmed FILE, not a selector for
//! whichever node happens to be current later. An old fixed-node read and
//! a new working edit use the same physical checks but different admission.
use super::{directory,directory_at,working_names,types::{Commit,FormalNode},physical::HistoryBody};
use crate::{FileId,NodeId,fs::{HeldDir,invalid},pool::lease::{Access,FileVolume,LocalPool},
    ordinary::{delta::{physical::Body,types::Delta},draft::types::SaveFrame,journal::{Journal,cycle::{Cycle,Parent}}}};
use std::{io::{self,Write},path::Path};

/// Own the SYSTEM lease and the exact journal it admitted together. Callers
/// cannot exchange either half for a stale snapshot or some other pool.
pub(in crate::ordinary) struct ParentCut{pool:LocalPool,journal:Journal}
impl ParentCut{
    pub(in crate::ordinary) fn open(system:&Path,access:Access)->io::Result<Self>{
        let(pool,_,journal)=LocalPool::with_snapshot(system,access)?;Ok(Self{pool,journal})
    }
    pub(in crate::ordinary) fn originals(&self)->(&LocalPool,&Journal){(&self.pool,&self.journal)}
    pub(in crate::ordinary) fn file(&self,id:FileId)->io::Result<Selected<'_>>{
        let cycle=self.journal.confirmed_file_cycle(id)?;
        let first=self.journal.saved(id).ok_or_else(||invalid("formal FILE lacks its original save"))?;
        Ok(Selected{cut:self,cycle,first})
    }
    pub(super) fn historical(&self,file:FileId,node:NodeId)->io::Result<PinnedParent>{
        if self.journal.node(file,node).is_none()&&self.journal.delta_node(file,node).is_none(){
            return Err(io::Error::new(io::ErrorKind::NotFound,"no such published node in this FILE"));
        }
        self.file(file)?.historical(node)
    }
}
pub(in crate::ordinary) struct Selected<'a>{cut:&'a ParentCut,cycle:Cycle<'a>,first:&'a SaveFrame}
impl<'a> Selected<'a>{
    pub(in crate::ordinary) fn cycle(&self)->&Cycle<'a>{&self.cycle}
    fn bind(&self,dir:&HeldDir,f:&Commit,d:Option<&Delta>)->io::Result<PinnedParent>{
        let site=self.cycle.placement();let first=site.full(dir,f)?.user_read()?;
        let (key,receipt,body)=match d{
            None=>(Parent::full(f),f.receipt(),Material::Full(first)),
            Some(child)=>(Parent::incremental(child),child.receipt(),Material::Incr(site.incremental(dir,first,child)?)),
        };let result=PinnedParent{key,receipt,body};result.require(dir)?;Ok(result)
    }
    pub(in crate::ordinary) fn working(&self,want:Parent)->io::Result<(FileVolume<'a>,HeldDir,PinnedParent)>{
        if want!=self.cycle.parent(){return Err(invalid("old or unrelated formal parent is not the original working baseline"));}
        let(f,d)=self.cycle.selected(want.node)?;let site=self.cycle.placement();
        let v=self.cut.pool.ordinary(site.volume())?;let dir=directory(&v,&self.cut.journal,site.birth(),self.first)?;
        let material=self.bind(&dir,f,d)?;
        if material.key()!=want{return Err(invalid("wrong native node for the captured FILE baseline"));}
        v.require_current()?;Ok((v,dir,material))
    }
    fn historical(&self,node:NodeId)->io::Result<PinnedParent>{
        let(f,d)=self.cycle.selected(node)?;let site=self.cycle.placement();let s=&self.cut.journal;
        let c=site.birth();let v=self.cut.pool.ordinary(site.volume())?;
        // Fixed FULL does not require a later working extent or native child
        // just because its name was issued. Never read versions from a draft.
        let child=s.delta(c.create.file).map(|x|(&x.original,d.is_some())).or_else(||s.incrementing.as_ref()
            .filter(|a|a.original.file==c.create.file).map(|a|(&a.original,false)));
        let dir=directory_at(&v,s,c,working_names(s,c,self.first,true)?,Some((&f.original,true)),child)?;
        let material=self.bind(&dir,f,d)?;v.root.file.sync_all()?;v.require_current()?;Ok(material)
    }
}
enum Material{Full(HistoryBody),Incr(Body)}
/// Holds leases on the exact native node/index, FULL and, only when selected,
/// INCR. It has no raw-directory access and grants no new write or custody.
pub(in crate::ordinary) struct PinnedParent{key:Parent,receipt:FormalNode,body:Material}
impl PinnedParent{
    pub(in crate::ordinary) fn key(&self)->Parent{self.key}
    pub(in crate::ordinary) fn unchanged(&self)->io::Result<()>{match &self.body{Material::Full(b)=>b.unchanged(),Material::Incr(b)=>b.unchanged()}}
    pub(in crate::ordinary) fn require(&self,dir:&HeldDir)->io::Result<()>{match &self.body{Material::Full(b)=>b.require(dir),Material::Incr(b)=>b.require(dir)}}
    pub(in crate::ordinary) fn write_range(&self,at:u64,n:u64,out:impl Write)->io::Result<()>{match &self.body{
        Material::Full(b)=>b.write_range(at,n,out),Material::Incr(b)=>b.range(at,n,out)
    }}
    pub(super) fn write_to(&self,out:impl Write)->io::Result<()>{match &self.body{Material::Full(b)=>b.write_to(out),Material::Incr(b)=>b.whole(out)}}
    pub(super) fn finish_output(self,out:&mut impl Write)->io::Result<FormalNode>{
        crate::ordinary::output::CheckedOutput{out,check:||self.unchanged()}.flush()?;Ok(self.receipt)
    }
}
