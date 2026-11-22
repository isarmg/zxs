//! In-memory projection of confirmed SYSTEM names. There is no store or
//! independent selector here. An occupied destination is enough to refuse;
//! withdrawing an ordinary source requires its exact confirmed FILE identity.
use super::MoveOrigin;
use crate::ordinary::custody::transfer::ParentEnd as End;
use crate::{EntryId,FileId,VolumeId,identity::DirectoryId,fs::invalid};
use std::{collections::BTreeMap,io};

use crate::ordinary::directory::types::Parent;
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) enum Member{File{entry:EntryId,file:FileId},Directory{entry:EntryId,directory:DirectoryId}}

#[derive(Default)]
pub(super) struct Names{
    parents:BTreeMap<Parent,BTreeMap<String,Member>>,
    // Derived, in-memory only. A FILE displayed like someone else's native
    // leaf is not itself a collision: compare each side's actual leaf.
    leaves:BTreeMap<Parent,BTreeMap<Vec<u8>,Member>>,
}
impl Member{
    fn leaf(self,name:&str)->io::Result<Vec<u8>>{
        use crate::ordinary::storage_name::{DirectoryName,file_leaf};
        Ok(match self{Self::File{file,..}=>file_leaf(name,file)?.into_bytes(),
            Self::Directory{..}=>DirectoryName::parse(name.as_bytes())?.leaf().into_bytes()})
    }
}
impl Names{
    pub(super) fn get(&self,p:Parent,name:&str)->Option<Member>{self.parents.get(&p)?.get(name).copied()}
    pub(super) fn root(&self,volume:VolumeId,name:&str)->Option<Member>{self.get(Parent::Root(volume),name)}
    pub(super) fn occupied_root(&self,volume:VolumeId,name:&str)->bool{self.root(volume,name).is_some()}
    pub(super) fn vacancy(&self,p:Parent,name:&str,member:Member)->io::Result<()>{
        let leaf=member.leaf(name)?;
        if self.get(p,name).is_some()||self.leaves.get(&p).is_some_and(|s|s.contains_key(&leaf)){
            return Err(io::Error::new(io::ErrorKind::AlreadyExists,"original business name or actual derived leaf is occupied"));
        }Ok(())
    }
    fn enter(&mut self,p:Parent,name:&str,e:Member)->io::Result<()>{
        self.vacancy(p,name,e)?;let leaf=e.leaf(name)?;
        self.parents.entry(p).or_default().insert(name.to_owned(),e);self.leaves.entry(p).or_default().insert(leaf,e);Ok(())
    }
    fn withdraw(&mut self,p:Parent,name:&str,expected:Member)->io::Result<()>{
        let leaf=expected.leaf(name)?;
        if self.get(p,name)!=Some(expected)||self.leaves.get(&p).and_then(|s|s.get(&leaf)).copied()!=Some(expected){
            return Err(invalid("source is not the exact original business entry and physical leaf of this parent"));}
        let siblings=self.parents.get_mut(&p).expect("checked parent");siblings.remove(name);
        if siblings.is_empty(){self.parents.remove(&p);}
        let physical=self.leaves.get_mut(&p).expect("checked physical parent");physical.remove(&leaf);
        if physical.is_empty(){self.leaves.remove(&p);}Ok(())
    }
    pub(super) fn add_directory(&mut self,d:&super::DirCreated)->io::Result<()>{
        let o=&d.origin;self.enter(o.parent.address,&o.name,Member::Directory{entry:o.entry,directory:o.id})
    }
    pub(super) fn hide_directory(&mut self,d:&super::DirCreated)->io::Result<()>{
        let o=&d.origin;self.withdraw(o.parent.address,&o.name,Member::Directory{entry:o.entry,directory:o.id})
    }
    pub(super) fn create_file(&mut self,parent:Parent,name:&str,entry:EntryId,file:FileId)->io::Result<()>{
        self.enter(parent,name,Member::File{entry,file})
    }
    pub(super) fn hide_created(&mut self,parent:Parent,name:&str,entry:EntryId,file:FileId)->io::Result<()>{
        self.withdraw(parent,name,Member::File{entry,file})
    }
    fn endpoints(&self,from:End,to:End,name:&str,file:Member)->io::Result<()>{
        if from.volume()==to.volume()||!matches!(file,Member::File{..}){return Err(invalid("different registered custody volumes and an original FILE required"));}
        let leaf=file.leaf(name)?;
        let member_at=|p|self.leaves.get(&p).and_then(|s|s.get(&leaf)).copied();
        // Both the business spelling and the native container leaf must
        // name this exact FILE. A recycle root never acquires an ordinary row.
        match from{
            End::Ordinary(p) if self.get(p,name)==Some(file)&&member_at(p)==Some(file)=>{},
            End::Recycle(v) if !self.occupied_root(v,name)&&member_at(Parent::Root(v)).is_none()=>{},
            _=>return Err(invalid("source does not own this exact original FILE entry and native leaf")),
        }
        match to{
            End::Ordinary(p)=>self.vacancy(p,name,file)?,
            End::Recycle(v) if !self.occupied_root(v,name)&&member_at(Parent::Root(v)).is_none()=>{},
            _=>return Err(invalid("dedicated recycle endpoint acquired an ordinary entry")),
        }
        Ok(())
    }
    pub(super) fn require_handoff(&self,o:&MoveOrigin)->io::Result<()>{
        let(from,to)=o.ends()?;self.endpoints(from,to,&o.name,Member::File{entry:o.entry,file:o.file})
    }
    /// Undo just the exact complete-but-unconfirmed candidate. R is never
    /// inserted. Parent-local names and private tickets stay where issued.
    pub(super) fn apply_handoff(&mut self,o:&MoveOrigin,undo:bool)->io::Result<()>{
        let(src,dst)=o.ends()?;let(from,to)=if undo{(dst,src)}else{(src,dst)};
        self.apply_pair(from,to,&o.name,Member::File{entry:o.entry,file:o.file})
    }
    fn apply_pair(&mut self,from:End,to:End,name:&str,file:Member)->io::Result<()>{
        self.endpoints(from,to,name,file)?;
        if let End::Ordinary(p)=from{self.withdraw(p,name,file)?;}
        if let End::Ordinary(p)=to{self.enter(p,name,file)?;}
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/attached/ordinary/journal/names/tests.rs"]
mod tests;
