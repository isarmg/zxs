//! In-memory projection of confirmed SYSTEM names. There is no store or
//! independent selector here. An occupied destination is enough to refuse;
//! withdrawing an ordinary source requires its exact confirmed FILE identity.
use super::{MoveOrigin,active_names};
use crate::{EntryId,FileId,VolumeId,identity::DirectoryId,fs::invalid};
use std::{collections::BTreeMap,io};

use crate::ordinary::directory::types::Parent;
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) enum Member{File{entry:EntryId,file:FileId},Directory{entry:EntryId,directory:DirectoryId}}

#[derive(Default)]
pub(super) struct Names{parents:BTreeMap<Parent,BTreeMap<String,Member>>}
impl Names{
    pub(super) fn get(&self,p:Parent,name:&str)->Option<Member>{self.parents.get(&p)?.get(name).copied()}
    pub(super) fn root(&self,volume:VolumeId,name:&str)->Option<Member>{self.get(Parent::Root(volume),name)}
    pub(super) fn occupied_root(&self,volume:VolumeId,name:&str)->bool{self.root(volume,name).is_some()}
    fn enter(&mut self,p:Parent,name:&str,e:Member)->io::Result<()>{
        use std::collections::btree_map::Entry;
        match self.parents.entry(p).or_default().entry(name.to_owned()){
            Entry::Vacant(v)=>{v.insert(e);Ok(())}
            Entry::Occupied(_)=>Err(invalid("ordinary parent already has an original entry with this name")),
        }
    }
    fn withdraw(&mut self,p:Parent,name:&str,expected:Member)->io::Result<()>{
        let siblings=self.parents.get_mut(&p).ok_or_else(||invalid("missing original parent of the confirmed entry"))?;
        if siblings.get(name)!=Some(&expected){return Err(invalid("source is not the exact original entry of this parent"));}
        siblings.remove(name);if siblings.is_empty(){self.parents.remove(&p);}Ok(())
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
    fn endpoints(&self,from:VolumeId,to:VolumeId,name:&str,expected:Option<Member>)->io::Result<()>{
        if from==to||self.root(from,name)!=expected||self.occupied_root(to,name){
            return Err(invalid("ordinary source is not this registered FILE entry or destination name is occupied"));
        }Ok(())
    }
    pub(super) fn require_handoff(&self,o:&MoveOrigin)->io::Result<()>{
        let(src,_)=active_names(o);
        self.endpoints(o.from,o.to,&o.name,src.then_some(Member::File{entry:o.entry,file:o.file}))
    }
    /// A dedicated endpoint never contributes an ordinary entry. Undo is
    /// confined to the one exact complete-but-unconfirmed candidate already
    /// authenticated by the preceding floor, intent and original reservation.
    pub(super) fn apply_handoff(&mut self,o:&MoveOrigin,undo:bool)->io::Result<()>{
        let(src,dst)=active_names(o);
        let(from,to,remove,add)=if undo{(o.to,o.from,dst,src)}else{(o.from,o.to,src,dst)};
        let file=Member::File{entry:o.entry,file:o.file};
        self.endpoints(from,to,&o.name,remove.then_some(file))?;
        if remove{self.withdraw(Parent::Root(from),&o.name,file)?;}
        if add{self.enter(Parent::Root(to),&o.name,file)?;}
        Ok(())
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn occupation_cannot_authorize_withdrawal_and_equal_names_are_scoped_to_their_real_parent(){
        let a=VolumeId([1;16]);let b=VolumeId([2;16]);let e=EntryId([3;16]);let f=FileId([4;16]);
        let g=DirectoryId([5;16]);let h=DirectoryId([6;16]);let other=EntryId([7;16]);
        let file=Member::File{entry:e,file:f};let dir=Member::Directory{entry:other,directory:g};
        let mut n=Names::default();n.enter(Parent::Root(a),"report",dir).unwrap();
        assert!(n.occupied_root(a,"report"));assert!(n.endpoints(a,b,"report",Some(file)).is_err());
        assert!(n.create_file(Parent::Root(a),"report",e,f).is_err());assert!(n.hide_created(Parent::Root(a),"report",e,f).is_err());
        n.create_file(Parent::Child(a,g),"report",e,f).unwrap();n.enter(Parent::Child(a,h),"report",dir).unwrap();
        assert_eq!(n.get(Parent::Child(a,g),"report"),Some(file));assert_eq!(n.get(Parent::Child(a,h),"report"),Some(dir));
        assert_eq!(n.get(Parent::Child(b,g),"report"),None);assert_eq!(n.root(a,"report"),Some(dir));
        assert!(n.hide_created(Parent::Child(a,h),"report",e,f).is_err());
        n.hide_created(Parent::Child(a,g),"report",e,f).unwrap();
        assert_eq!(n.get(Parent::Child(a,g),"report"),None);
        assert_eq!(n.get(Parent::Child(a,h),"report"),Some(dir));assert_eq!(n.root(a,"report"),Some(dir));
        n.withdraw(Parent::Root(a),"report",dir).unwrap();n.create_file(Parent::Root(a),"report",e,f).unwrap();
        assert!(n.endpoints(a,b,"report",Some(Member::File{entry:other,file:f})).is_err());
        assert!(n.endpoints(a,b,"report",Some(Member::File{entry:e,file:FileId([8;16])})).is_err());
        n.endpoints(a,b,"report",Some(file)).unwrap();n.enter(Parent::Root(b),"report",dir).unwrap();
        assert!(n.endpoints(a,b,"report",Some(file)).is_err());assert_eq!(n.root(b,"report"),Some(dir));
    }
}
