//! Same recorded INIT role at an actual, independent runtime source. An
//! absent or uncertain root has no placeholder mount and no authority at all.
use super::bad;
use crate::{maintenance::HostMounts,pinned::{Entity,Kind,Mount,Pinned},
    sqlite_install::{catalog::Initial,markers::RecycleMark,originals::named_backing}};
use rustix::fs::{StatVfsMountFlags,fstatvfs};
use std::{fs::File,io,os::unix::fs::MetadataExt,path::Path};

fn no()->io::Error{bad("cannot prove the original independent recycle disk")}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct Identity{entity:Entity,mount:Mount,backing:u64}
impl Identity{
    fn actual(pin:&Pinned,file:&File,table:&HostMounts)->io::Result<Self>{
        let entity=Entity::of(&file.metadata()?)?;
        let mount=Mount::on(file)?.ok_or_else(no)?;
        if entity!=pin.entity()||mount!=pin.require_same_named_mount()?{return Err(no());}
        Ok(Self{entity,mount,backing:table.full_mount(mount.id)?.1})
    }
    fn separate(self,other:Self)->bool{
        self.entity.dev!=other.entity.dev&&self.mount.id!=other.mount.id&&self.backing!=other.backing
    }
}
fn disjoint(a:&Pinned,b:&Pinned)->bool{
    a.entity()!=b.entity()&&!a.entity_is_visible_ancestor_of(b)&&!b.entity_is_visible_ancestor_of(a)
}
fn validate_source<'a>(pin:&Pinned,dir:&File,host:&File,uid:u32,
    other:impl IntoIterator<Item=(&'a Pinned,&'a File)>)->io::Result<Identity>{
    let m=dir.metadata()?;
    if m.mode()!=0o040_700||m.uid()!=uid||m.nlink()==0||fstatvfs(dir)?.f_flag.contains(StatVfsMountFlags::RDONLY){
        return Err(no());
    }
    let table=HostMounts::current()?;let r=Identity::actual(pin,dir,&table)?;
    if named_backing(pin,dir,&table,host,uid)?!=r.backing{return Err(no());}
    for (p,d) in other {
        if !disjoint(pin,p)||!r.separate(Identity::actual(p,d,&table)?){return Err(no());}
    }
    Ok(r)
}

pub(super) struct Recycle{pin:Pinned,dir:File,mark:RecycleMark,wanted:[u8;101],identity:Identity}
impl Recycle{
    pub(super) fn select<'a>(claim:&Initial,host:&File,uid:u32,
        other:impl IntoIterator<Item=(&'a Pinned,&'a File)>)->io::Result<Self>{
        Self::selected(Pinned::already_mounted(&claim.path,Kind::StorageRoot)?,claim,host,uid,other)
    }
    fn selected<'a>(pin:Pinned,claim:&Initial,host:&File,uid:u32,
        other:impl IntoIterator<Item=(&'a Pinned,&'a File)>)->io::Result<Self>{
        if pin.path()!=claim.path.as_path()||claim.marker[20]!=1{return Err(no());}
        let dir=pin.storage_directory()?;
        let identity=validate_source(&pin,&dir,host,uid,other)?;
        // Only the bounded, protected 101-byte marker is read. The business
        // contents, old/foreign names, pathname aliases and devices are not.
        let mark=RecycleMark::select(&dir,uid,&claim.marker)?;
        if Identity::actual(&pin,&dir,&HostMounts::current()?)?!=identity{return Err(no());}
        Ok(Self{pin,dir,mark,wanted:claim.marker,identity})
    }
    pub(super) fn require<'a>(&self,host:&File,uid:u32,
        other:impl IntoIterator<Item=(&'a Pinned,&'a File)>)->io::Result<()>{
        if validate_source(&self.pin,&self.dir,host,uid,other)?!=self.identity{return Err(no());}
        self.mark.verify(&self.dir,&self.wanted)?;
        if self.pin.require_same_named_mount()?!=self.identity.mount{return Err(no());}
        Ok(())
    }
    pub(super) fn source(&self)->(&Path,&File){(self.pin.path(),&self.dir)}
    pub(super) fn pinned(&self)->&Pinned{&self.pin}
}

#[cfg(test)]
mod tests{
    use super::*;
    fn key(dev:u64,mount:u64,backing:u64)->Identity{
        Identity{entity:Entity{dev,ino:4},mount:Mount{id:mount,at_root:Some(true)},backing}
    }
    #[test]fn all_three_backing_observations_must_be_separate(){
        let a=key(1,2,3);assert!(a.separate(key(4,5,6)));
        for reused in [key(1,5,6),key(4,2,6),key(4,5,3)]{assert!(!a.separate(reused));}
    }
}
