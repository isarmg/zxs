//! The actual parent material of one confirmed FILE, not a selector for
//! whichever node happens to be current later. An old fixed-node read and
//! a new working edit use the same physical checks but different admission.
use super::{child_names,directory,directory_at,working_names,types::FormalNode,physical::HistoryBody};
use crate::{FileId,NodeId,fs::{HeldDir,invalid},pool::lease::{Access,FileVolume,LocalPool},
    ordinary::{delta::{physical::Body,types::Delta},draft::types::SaveFrame,wire::NativeObject,custody::transfer::ChildRoles,placement::Placement,journal::{Graph,Journal,cycle::{Cycle,Parent}}}};
use std::{io::{self,Write},path::Path};

/// Own the SYSTEM lease and the exact journal it admitted together. Callers
/// cannot exchange either half for a stale snapshot or some other pool.
pub(in crate::ordinary) struct ParentCut{pool:LocalPool,journal:Journal}
impl ParentCut{
    pub(in crate::ordinary) fn open(system:&Path,access:Access)->io::Result<Self>{
        let(pool,_,journal)=LocalPool::with_snapshot(system,access)?;Ok(Self{pool,journal})
    }
    pub(in crate::ordinary) fn capture<T>(system:&Path,choose:impl FnOnce(&Self)->io::Result<T>)
        ->io::Result<crate::pool::lease::RegisteredRead<T>>{
        LocalPool::capture_read(system,|pool,journal|choose(&Self{pool,journal}))
    }
    pub(in crate::ordinary) fn originals(&self)->(&LocalPool,&Journal){(&self.pool,&self.journal)}
    pub(in crate::ordinary) fn file(&self,id:FileId)->io::Result<Selected<'_>>{
        Selected::from_cut(&self.pool,&self.journal,id)
    }
    pub(super) fn historical(&self,file:FileId,node:NodeId)->io::Result<PinnedParent>{
        if self.journal.node(file,node).is_none()&&self.journal.delta_node(file,node).is_none(){
            return Err(io::Error::new(io::ErrorKind::NotFound,"no such published node in this FILE"));
        }
        self.file(file)?.historical(node)
    }
}
pub(in crate::ordinary) struct Selected<'a>{pool:&'a LocalPool,journal:&'a Journal,graph:Graph<'a>,site:Placement<'a>,first:&'a SaveFrame}
impl<'a> Selected<'a>{
    fn from_cut(pool:&'a LocalPool,journal:&'a Journal,id:FileId)->io::Result<Self>{
        let graph=journal.actual_graph(id)?;let site=journal.placement(id)?;
        let first=journal.saved(id).ok_or_else(||invalid("formal FILE lacks its original save"))?;
        Ok(Self{pool,journal,graph,site,first})
    }
    pub(in crate::ordinary) fn parent(&self)->Parent{self.graph.tip()}
    pub(in crate::ordinary) fn placement(&self)->Placement<'a>{self.site}
    // Only original working admission needs the complete dated placement/change walk.
    // A volume listing instead uses its already-confirmed FILE and child indexes.
    pub(in crate::ordinary) fn cycle(&self)->io::Result<Cycle<'a>>{
        let c=self.journal.confirmed_file_cycle(self.site.birth().create.file)?;
        if c.parent()!=self.parent()||c.placement().key()!=self.site.key(){return Err(invalid("working cycle conflicts with confirmed original FILE graph"));}
        Ok(c)
    }
    /// Private to this original cut. Callers cannot provide a directory,
    /// detached historical Graph, or alternate native object when selecting
    /// a new working baseline or a fixed historical read.
    fn bind(&self,v:&FileVolume<'_>,dir:&HeldDir,node:NodeId)->io::Result<PinnedParent>{
        let graph=self.graph.through(node)?;let f=graph.full();let site=self.site;v.require_current()?;
        if !std::ptr::eq(v.pool,self.pool)||v.id()!=site.volume(){return Err(invalid("wrong registered volume for original FILE graph"));}
        crate::ordinary::material::verify_header_at(dir,&site.birth().create,site.directory(),site.local())?;
        let base=site.full(dir,f)?.user_read()?;
        let (receipt,body)=match graph.children(){
            ChildRoles::None=>(f.receipt(),Material::Full(base)),
            ChildRoles::One(d)=>(d.receipt(),Material::Incr(Body::at_original(dir,base,d,site.active_child(graph,0)?)?)),
            ChildRoles::Two(d,n)=>{
                let parent=Body::at_original(dir,base,d,site.active_child(graph,0)?)?;
                let target=PlacedSecond{dir,parent:Parent::incremental(d),child:n,object:site.active_child(graph,1)?};
                (n.receipt(),Material::Incr(parent.extend(&target)?))
            }
        };let result=PinnedParent{key:graph.tip(),receipt,body};result.require(dir)?;v.require_current()?;Ok(result)
    }
    /// Reuse a caller's *same* volume lease, not its arbitrary directory.
    /// Select a genuine formal baseline while checking the entire currently
    /// issued directory (including a possible sealed yet unconfirmed child).
    pub(in crate::ordinary) fn on_volume(&self,v:&FileVolume<'_>,want:Parent)->io::Result<(HeldDir,PinnedParent)>{
        let g=self.graph.through(want.node)?;let site=self.site;
        if g.tip()!=want||!std::ptr::eq(v.pool,self.pool)||v.id()!=site.volume(){return Err(invalid("different original formal FILE/owner/volume at this cut"));}
        let dir=directory(v,self.journal,site.birth(),self.first)?;let material=self.bind(v,&dir,want.node)?;
        if material.key()!=want{return Err(invalid("different selected original working parent"));}
        v.require_current()?;Ok((dir,material))
    }
    pub(in crate::ordinary) fn working(&self,want:Parent)->io::Result<(FileVolume<'a>,HeldDir,PinnedParent)>{
        if want!=self.parent(){return Err(invalid("old or unrelated formal parent is not the original working baseline"));}
        let v=self.pool.ordinary(self.site.volume())?;
        let(dir,material)=self.on_volume(&v,want)?;Ok((v,dir,material))
    }
    fn historical(&self,node:NodeId)->io::Result<PinnedParent>{
        let graph=self.graph.through(node)?;let site=self.site;let s=self.journal;
        let c=site.birth();let v=self.pool.ordinary(site.volume())?;
        let ds=child_names(s,c.create.file,Some(node))?;
        let dir=directory_at(&v,s,c,working_names(s,c,self.first,true)?,Some((&graph.full().original,true)),&ds)?;
        let material=self.bind(&v,&dir,node)?;v.root.file.sync_all()?;v.require_current()?;Ok(material)
    }
}
/// Reopen the newly *confirmed* node under the current writer before
/// retiring its spent working extents. No caller supplies a Journal, parent
/// inode or alternate FILE route; the slot must still be this original.
pub(in crate::ordinary) fn for_retirement(v:&FileVolume<'_>,old_dir:&HeldDir,d:&Delta)->io::Result<PinnedParent>{
    let p=v.pool;if !matches!(p.access,Access::Write){return Err(invalid("not the original retirement writer"));}
    let(_,s)=p.snapshot()?;
    if !s.incrementing.as_ref().is_some_and(|a|a.matches(d))||s.decided(d.original.op).map(|f|f.delta()).transpose()?!=Some(d){
        return Err(invalid("unconfirmed or unrelated node cannot release working material"));}
    let sf=Selected::from_cut(p,&s,d.original.file)?;
    let want=Parent::incremental(d);if sf.parent()!=want{return Err(invalid("retirement is not the acknowledged original FILE front"));}
    let(dir,parent)=sf.on_volume(v,want)?;
    if dir.entity!=old_dir.entity||!dir.same_mount(old_dir)?{return Err(invalid("different original named FILE for working cleanup"));}
    old_dir.require_named()?;parent.require(old_dir)?;Ok(parent)
}
/// Born inside Selected::bind from the same current SYSTEM snapshot, original
/// registered volume and actual FILE directory. No writable capability.
pub(in crate::ordinary) struct PlacedSecond<'a>{dir:&'a HeldDir,parent:Parent,child:&'a Delta,object:&'a NativeObject}
impl<'a> PlacedSecond<'a>{
    pub(in crate::ordinary) fn parts(&self)->(&'a HeldDir,Parent,&'a Delta,&'a NativeObject){(self.dir,self.parent,self.child,self.object)}
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
    pub(in crate::ordinary) fn write_to(&self,out:impl Write)->io::Result<()>{match &self.body{Material::Full(b)=>b.write_to(out),Material::Incr(b)=>b.whole(out)}}
    pub(super) fn receipt(&self)->&FormalNode{&self.receipt}
    pub(super) fn into_receipt(self)->FormalNode{self.receipt}
    pub(super) fn finish_output(&self,out:&mut impl Write)->io::Result<()>{
        crate::ordinary::output::CheckedOutput{out,check:||self.unchanged()}.flush()
    }
}
