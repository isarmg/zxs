//! Held ordinary directory lineage. The caller supplies a registered FILE
//! volume and a typed address, never a host pathname or detached parent frame.
use super::types::{Basis,Created,Parent};
use crate::{fs::{self,HeldDir,invalid},ordinary::{journal::Journal,material},pool::lease::FileVolume};
use std::{collections::BTreeSet,ffi::{CStr,CString},io};

// A standalone operation owns its registered volume; an operation already
// holding that volume borrows it. Either way the one original root lease
// outlives every parent directory lease, without reopening that root flock.
enum Volume<'v,'p>{Borrowed(&'v FileVolume<'p>),Owned(FileVolume<'p>)}
impl<'p> Volume<'_,'p>{fn get(&self)->&FileVolume<'p>{match self{Self::Borrowed(v)=>v,Self::Owned(v)=>v}}}
/// Bounded to one read-only volume lease, a single original parent, and a
/// single name census. The directory FD is a duplicate of the already-leased
/// original, not a fresh path-based selection.
pub(crate) struct NameSample{basis:Basis,frontier:fs::Frontier,dir:HeldDir,names:BTreeSet<Vec<u8>>}
impl NameSample{
    fn held<'a>(v:&'a FileVolume<'_>,address:Parent,cut:&Journal)->io::Result<Option<&'a Self>>{
        let Some(s)=v.listing.get()else{return Ok(None)};
        if s.basis.address!=address||s.frontier!=cut.confirmed_floor()||v.id()!=address.volume()||!matches!(v.pool.access,crate::pool::lease::Access::Read){
            return Err(invalid("a direct name sample cannot describe another parent or writer"));}
        s.dir.require_named()?;Ok(Some(s))
    }
    pub(in crate::ordinary) fn names(&self)->&BTreeSet<Vec<u8>>{&self.names}
}
/// One address selection for local births and relocated FILEs. The caller
/// checks the FILE-specific original against this exact directory basis.
pub(in crate::ordinary) fn read_parent<T>(v:&FileVolume<'_>,s:&Journal,address:Parent,
    action:impl FnOnce(&Basis,&HeldDir,Option<&BTreeSet<Vec<u8>>>)->io::Result<T>)->io::Result<T>{
    if let Some(x)=NameSample::held(v,address,s)?{
        let result=action(&x.basis,&x.dir,Some(&x.names))?;x.dir.require_named()?;return Ok(result);
    }
    let h=HeldParent::at_cut(v,address,s)?;
    let result=action(h.basis(),h.directory(),None)?;h.require_at(s)?;Ok(result)
}
pub(in crate::ordinary) fn direct_child(dir:&HeldDir,name:&CStr,sample:Option<&BTreeSet<Vec<u8>>>)->io::Result<Option<HeldDir>>{
    match sample{Some(n)=>dir.directory_in_sample(name,n),None=>dir.exact_directory(name)}
}
pub(crate) struct HeldParent<'v,'p>{v:Volume<'v,'p>,address:Parent,basis:Basis,chain:Vec<(Created,HeldDir)>}
impl<'v,'p> HeldParent<'v,'p>{
    pub(in crate::ordinary) fn open(v:&'v FileVolume<'p>,address:Parent)->io::Result<Self>{
        if v.id()!=address.volume(){return Err(invalid("directory address is not on this registered ordinary volume"));}
        let(_,s)=v.pool.snapshot()?;let result=Self::at_cut(v,address,&s)?;result.require_at(&s)?;Ok(result)
    }
    pub(in crate::ordinary) fn at_cut(v:&'v FileVolume<'p>,address:Parent,s:&Journal)->io::Result<Self>{
        Self::capture(Volume::Borrowed(v),address,s)
    }
    pub(crate) fn owned(p:&'p crate::pool::lease::LocalPool,address:Parent,s:&Journal)->io::Result<Self>{
        Self::capture(Volume::Owned(p.ordinary(address.volume())?),address,s)
    }
    fn capture(owner:Volume<'v,'p>,address:Parent,s:&Journal)->io::Result<Self>{
        let v=owner.get();let proof=s.file_parent_at(address,fs::local_uid(),&v.pool.journal_registration())?;
        let mut basis=Basis::registered(v)?;let mut chain:Vec<(Created,HeldDir)>=Vec::new();
        for row in s.directory_lineage(address)?{
            let previous=chain.last().map(|(d,_)|d);
            row.relation(&v.pool.journal_registration(),previous)?;
            if s.making.as_ref().is_some_and(|a|a.origin.id==row.origin.id){return Err(invalid("original mkdir has not finished installing this parent"));}
            if row.origin.parent!=basis{return Err(invalid("directory did not descend from its exact selected original parent"));}
            let parent=chain.last().map_or(&v.root,|(_,h)|h);
            let held=installed(parent,row)?;
            match v.pool.access {crate::pool::lease::Access::Write=>held.exclusive()?,_=>held.share()?}
            held.require_named()?;
            basis=Basis{address:Parent::Child(v.id(),row.origin.id),anchor:row.digest,object:row.object.clone()};
            chain.push((row.clone(),held));
        }
        if basis.address!=address||!proof.identifies(v.id(),&basis){return Err(invalid("original directory chain did not reach the selected registered address"));}
        Ok(Self{v:owner,address,basis,chain})
    }
    pub(crate) fn volume(&self)->&FileVolume<'p>{self.v.get()}
    pub(in crate::ordinary) fn basis(&self)->&Basis{&self.basis}
    pub(in crate::ordinary) fn address(&self)->Parent{self.address}
    pub(crate) fn directory(&self)->&HeldDir{self.chain.last().map_or(&self.volume().root,|(_,h)|h)}
    pub(in crate::ordinary) fn begin_listing(&self,s:&Journal)->io::Result<&NameSample>{
        let v=self.volume();if !matches!(v.pool.access,crate::pool::lease::Access::Read)||v.listing.get().is_some(){
            return Err(invalid("not a new read-only directory listing"));}
        self.require_at(s)?;let d=self.directory();
        let dir=d.duplicate()?;let names=dir.names()?.into_iter().collect();
        v.listing.set(NameSample{basis:self.basis.clone(),frontier:s.confirmed_floor(),dir,names}).map_err(|_|invalid("name sample already in use"))?;
        self.require_at(s)?;NameSample::held(v,self.address,s)?.ok_or_else(||invalid("missing original directory sample"))
    }
    pub(in crate::ordinary) fn finish_listing(&self,s:&Journal)->io::Result<()>{
        let sample=NameSample::held(self.volume(),self.address,s)?.ok_or_else(||invalid("no held name sample"))?;
        self.require_at(s)?;
        if sample.basis!=self.basis||sample.dir.names()?.into_iter().collect::<BTreeSet<_>>()!=sample.names{
            return Err(invalid("exact ordinary directory changed since its read-only census"));}
        self.require_at(s)
    }
    pub(in crate::ordinary) fn exact_child(&self,name:&CStr)->io::Result<Option<HeldDir>>{
        // Custody rechecks at each actual write/installation cut. That is not
        // an element lookup in the shared read-only direct-list snapshot.
        self.directory().exact_directory(name)
    }
    pub(in crate::ordinary) fn require(&self)->io::Result<()>{
        if matches!(self.address,Parent::Root(_)){return self.volume().require_current();}
        let(_,s)=self.volume().pool.snapshot()?;self.require_at(&s)
    }
    pub(crate) fn require_at(&self,s:&Journal)->io::Result<()>{
        self.volume().require_current()?;
        let rows=s.directory_lineage(self.address)?;if rows.len()!=self.chain.len(){return Err(invalid("different confirmed directory depth"));}
        let mut parent=&self.volume().root;
        for ((original,held),now) in self.chain.iter().zip(rows){
            if s.making.as_ref().is_some_and(|a|a.origin.id==now.origin.id){return Err(invalid("ordinary parent installation pending"));}
            if original!=now{return Err(invalid("physical directory is not its original confirmed entry"));}
            let actual=installed(parent,original)?;
            if actual.entity!=held.entity||fs::Birth::read(&held.file)?!=original.object.birth||!actual.same_mount(held)?{
                return Err(invalid("directory parent changed between original admission and use"));}
            held.require_named()?;parent=held;
        }
        self.volume().require_current()
    }
}
/// Checks a settled child, not its contents or the right to make a new
/// change. This directory may have had real authorized children since birth.
pub(super) fn installed(parent:&HeldDir,row:&Created)->io::Result<HeldDir>{
    installed_in(parent,row,None)
}
pub(super) fn installed_in(parent:&HeldDir,row:&Created,sample:Option<&std::collections::BTreeSet<Vec<u8>>>)->io::Result<HeldDir>{
    let o=&row.origin;
    if parent.entry_present(&material::staging(o.ticket,row.slot))?{return Err(invalid("original directory private slot is still occupied"));}
    let leaf=CString::new(o.name.as_bytes()).map_err(|_|invalid("invalid original directory name"))?;
    let child=match sample{Some(n)=>parent.directory_in_sample(&leaf,n)?,None=>parent.exact_directory(&leaf)?}.ok_or_else(||invalid("confirmed directory is not installed at its original name"))?;
    if child.entity!=row.object.entity||fs::Birth::read(&child.file)?!=row.object.birth||child.label()?.is_some(){
        return Err(invalid("not this original, unlabelled ordinary directory"));}
    child.require_named()?;Ok(child)
}

#[cfg(test)]
#[path="../../../tests/unit/directory_path.rs"]
mod tests;
