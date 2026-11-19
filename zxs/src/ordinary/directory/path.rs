//! Held ordinary directory lineage. The caller supplies a registered FILE
//! volume and a typed address, never a host pathname or detached parent frame.
use super::types::{Basis,Created,Parent};
use crate::{fs::{self,HeldDir,invalid},ordinary::{journal::Journal,material},pool::lease::FileVolume};
use std::{ffi::CString,io};

pub(super) struct HeldParent<'v,'p>{v:&'v FileVolume<'p>,address:Parent,basis:Basis,chain:Vec<(Created,HeldDir)>}
impl<'v,'p> HeldParent<'v,'p>{
    pub(super) fn open(v:&'v FileVolume<'p>,address:Parent)->io::Result<Self>{
        if v.id()!=address.volume(){return Err(invalid("directory address is not on this registered ordinary volume"));}
        let(_,s)=v.pool.snapshot()?;let result=Self::at_cut(v,address,&s)?;result.require_at(&s)?;Ok(result)
    }
    fn at_cut(v:&'v FileVolume<'p>,address:Parent,s:&Journal)->io::Result<Self>{
        let mut basis=Basis::registered(v)?;let mut chain:Vec<(Created,HeldDir)>=Vec::new();
        for row in s.directory_lineage(address)?{
            let previous=chain.last().map(|(d,_)|d);
            row.relation(&v.pool.journal_registration(),previous)?;
            if row.origin.parent!=basis{return Err(invalid("directory did not descend from its exact selected original parent"));}
            let parent=chain.last().map_or(&v.root,|(_,h)|h);
            let held=installed(parent,row)?;
            match v.pool.access {crate::pool::lease::Access::Write=>held.exclusive()?,_=>held.share()?}
            held.require_named()?;
            basis=Basis{address:Parent::Child(v.id(),row.origin.id),anchor:row.digest,object:row.object.clone()};
            chain.push((row.clone(),held));
        }
        if basis.address!=address{return Err(invalid("original directory chain did not reach selected address"));}
        Ok(Self{v,address,basis,chain})
    }
    pub(super) fn basis(&self)->&Basis{&self.basis}
    pub(super) fn directory(&self)->&HeldDir{self.chain.last().map_or(&self.v.root,|(_,h)|h)}
    pub(super) fn require(&self)->io::Result<()>{
        let(_,s)=self.v.pool.snapshot()?;self.require_at(&s)
    }
    fn require_at(&self,s:&Journal)->io::Result<()>{
        self.v.require_current()?;
        let rows=s.directory_lineage(self.address)?;if rows.len()!=self.chain.len(){return Err(invalid("different confirmed directory depth"));}
        let mut parent=&self.v.root;
        for ((original,held),now) in self.chain.iter().zip(rows){
            if original!=now{return Err(invalid("physical directory is not its original confirmed entry"));}
            let actual=installed(parent,original)?;
            if actual.entity!=held.entity||fs::Birth::read(&held.file)?!=original.object.birth||!actual.same_mount(held)?{
                return Err(invalid("directory parent changed between original admission and use"));}
            held.require_named()?;parent=held;
        }
        self.v.require_current()
    }
}
/// Checks a settled child, not its contents or the right to make a new
/// change. This directory may have had real authorized children since birth.
fn installed(parent:&HeldDir,row:&Created)->io::Result<HeldDir>{
    let o=&row.origin;
    if parent.entry_present(&material::staging(o.ticket,row.slot))?{return Err(invalid("original directory private slot is still occupied"));}
    let leaf=CString::new(o.name.as_bytes()).map_err(|_|invalid("invalid original directory name"))?;
    let child=parent.child_directory(&leaf)?.ok_or_else(||invalid("confirmed directory is not installed at its original name"))?;
    if child.entity!=row.object.entity||fs::Birth::read(&child.file)?!=row.object.birth||child.label()?.is_some(){
        return Err(invalid("not this original, unlabelled ordinary directory"));}
    child.require_named()?;Ok(child)
}

#[cfg(test)]
#[path="../../../tests/unit/directory_path.rs"]
mod tests;
