//! Current material at a confirmed FILE's unchanged birthplace. A previous
//! custody site, a detached birth frame or an occupied child is not a
//! lookup key. A moved/retained FILE needs its custody operation instead.
use super::path::{direct_child,read_parent};
use crate::{fs::{self,HeldDir,invalid},ordinary::{journal::Journal,material,wire::{Creation,Frame}},pool::lease::FileVolume};
use std::{collections::BTreeSet,io};

fn original(parent:&HeldDir,c:&Creation,sample:Option<&BTreeSet<Vec<u8>>>)->io::Result<HeldDir>{
    let name=crate::ordinary::storage_name::file_leaf(&c.name,c.file)?;
    let dir=direct_child(parent,&name,sample)?.ok_or_else(||invalid("current FILE container absent from its original ordinary parent"))?;
    material::verify_header_at(&dir,c,&c.directory,&c.local)?;
    if parent.entry_present(&material::staging(c.attempt,c.slot))?{return Err(invalid("both original FILE names present"));}
    Ok(dir)
}
pub(in crate::ordinary) fn open(v:&FileVolume<'_>,s:&Journal,expected:&Frame)->io::Result<HeldDir>{
    let birth=s.unchanged_birth(v.pool,v.id(),expected)?;let c=&birth.create;let address=c.parent.address(v.id());
    read_parent(v,s,address,|basis,parent,sample|{
        if !c.parent.identifies(v.id(),basis){return Err(invalid("FILE birth did not select this original directory"));}
        let dir=original(parent,c,sample)?;
        // A direct listing checks its original parent lease and syncs that
        // parent once before returning any entries. A single child lookup
        // proves durability here, with the same confirmed floor and objects.
        if sample.is_none()&&matches!(address,super::types::Parent::Child(..)){
            parent.file.sync_all()?;s.unchanged_birth(v.pool,v.id(),birth)?;
            let checked=original(parent,c,None)?;
            if checked.entity!=dir.entity||!dir.same_mount(&checked)?||fs::Birth::read(&dir.file)?!=c.directory.birth{
                return Err(invalid("different original child FILE after parent synchronization"));}
        }
        dir.require_named()?;Ok(dir)
    })
}

#[cfg(test)]
#[path="../../../tests/unit/ordinary_file_home.rs"]
mod tests;
