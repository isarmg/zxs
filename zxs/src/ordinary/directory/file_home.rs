//! Current material at a confirmed FILE's unchanged birthplace. A previous
//! custody site, a detached birth frame or an occupied child is not a
//! lookup key. A moved/retained FILE needs its custody operation instead.
use super::{path::HeldParent,types::Parent};
use crate::{fs::{self,HeldDir,invalid},ordinary::{file_parent::BirthParent,journal::Journal,material,wire::{Creation,Frame}},
    pool::lease::FileVolume};
use std::io;

fn original(v:&FileVolume<'_>,parent:&HeldDir,c:&Creation)->io::Result<HeldDir>{
    let name=crate::ordinary::storage_name::file_leaf(&c.name,c.file)?;
    let dir=if std::ptr::eq(parent,&v.root){v.exact_directory(&name)?}else{parent.exact_directory(&name)?}.ok_or_else(||invalid("current FILE container not installed at original parent"))?;
    material::verify_header_at(&dir,c,&c.directory,&c.local)?;
    if parent.child_directory(&material::staging(c.attempt,c.slot))?.is_some(){return Err(invalid("both original FILE names present"));}
    Ok(dir)
}
/// s and volume are the caller's held SYSTEM/volume cut. The root fast
/// path checks the original native stamp of the fully admitted bytes and its
/// 48-byte floor, without rereading the journal per entry. A changed stamp
/// requires another complete SYSTEM admission. A stale pre-move cut cannot
/// serve the old container. Child lookup reopens its exact ordinary ancestry.
pub(in crate::ordinary) fn open(v:&FileVolume<'_>,s:&Journal,expected:&Frame)->io::Result<HeldDir>{
    let reg=v.pool.journal_registration();let birth=s.unchanged_birth(v.pool,v.id(),expected)?;
    match &birth.create.parent{
        BirthParent::Root=>original(v,&v.root,&birth.create),
        BirthParent::Child{directory,decision,object}=>{
            let(_,now)=v.pool.snapshot()?;let active=now.unchanged_birth(v.pool,v.id(),birth)?;
            let address=Parent::Child(v.id(),*directory);
            if now.file_parent_at(address,active.create.uid,&reg)?!=active.create.parent{
                return Err(invalid("child FILE did not originate at this exact confirmed ordinary directory"));}
            let p=HeldParent::at_cut(v,address,&now)?;p.require_at(&now)?;
            if p.basis().anchor!=*decision||p.basis().object!=*object{
                return Err(invalid("child FILE parent is no longer its original ordinary directory"));}
            let dir=original(v,p.directory(),&active.create)?;
            // A visible final name alone does not prove its parent rename
            // durable. Sync the name's actual parent, not merely the volume.
            p.directory().file.sync_all()?;
            let(_,end)=v.pool.snapshot()?;p.require_at(&end)?;let f=end.unchanged_birth(v.pool,v.id(),birth)?;
            if end.file_parent_at(address,f.create.uid,&reg)?!=f.create.parent{
                return Err(invalid("different original parent after child FILE synchronization"));}
            let checked=original(v,p.directory(),&f.create)?;
            if dir.entity!=checked.entity||fs::Birth::read(&dir.file)?!=f.create.directory.birth||!dir.same_mount(&checked)?{
                return Err(invalid("different held child FILE container after parent check"));}
            dir.require_named()?;Ok(dir)
        }
    }
}

#[cfg(test)]
#[path="../../../tests/unit/ordinary_file_home.rs"]
mod tests;
