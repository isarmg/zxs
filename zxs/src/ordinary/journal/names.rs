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
mod tests{
    use super::*;
    #[test]
    fn occupation_cannot_authorize_withdrawal_and_equal_names_are_scoped_to_their_real_parent(){
        let a=VolumeId([1;16]);let b=VolumeId([2;16]);let e=EntryId([3;16]);let f=FileId([4;16]);
        let g=DirectoryId([5;16]);let h=DirectoryId([6;16]);let other=EntryId([7;16]);
        let file=Member::File{entry:e,file:f};let dir=Member::Directory{entry:other,directory:g};
        let(ea,eb)=(End::Ordinary(Parent::Root(a)),End::Ordinary(Parent::Root(b)));
        let mut n=Names::default();n.enter(Parent::Root(a),"report",dir).unwrap();
        assert!(n.occupied_root(a,"report"));assert!(n.endpoints(ea,eb,"report",file).is_err());
        assert!(n.create_file(Parent::Root(a),"report",e,f).is_err());assert!(n.hide_created(Parent::Root(a),"report",e,f).is_err());
        n.create_file(Parent::Child(a,g),"report",e,f).unwrap();n.enter(Parent::Child(a,h),"report",dir).unwrap();
        assert_eq!(n.get(Parent::Child(a,g),"report"),Some(file));assert_eq!(n.get(Parent::Child(a,h),"report"),Some(dir));
        assert_eq!(n.get(Parent::Child(b,g),"report"),None);assert_eq!(n.root(a,"report"),Some(dir));
        assert!(n.hide_created(Parent::Child(a,h),"report",e,f).is_err());
        n.hide_created(Parent::Child(a,g),"report",e,f).unwrap();
        assert_eq!(n.get(Parent::Child(a,g),"report"),None);
        assert_eq!(n.get(Parent::Child(a,h),"report"),Some(dir));assert_eq!(n.root(a,"report"),Some(dir));
        n.withdraw(Parent::Root(a),"report",dir).unwrap();n.create_file(Parent::Root(a),"report",e,f).unwrap();
        assert!(n.endpoints(ea,eb,"report",Member::File{entry:other,file:f}).is_err());
        assert!(n.endpoints(ea,eb,"report",Member::File{entry:e,file:FileId([8;16])}).is_err());
        n.endpoints(ea,eb,"report",file).unwrap();n.enter(Parent::Root(b),"report",dir).unwrap();
        assert!(n.endpoints(ea,eb,"report",file).is_err());assert_eq!(n.root(b,"report"),Some(dir));
    }
    #[test]
    fn both_directions_change_only_the_exact_direct_parents_never_an_ordinary_recycle_row(){
        let a=VolumeId([1;16]);let b=VolumeId([2;16]);let r=VolumeId([3;16]);
        let left=Parent::Child(b,DirectoryId([4;16]));let right=Parent::Child(b,DirectoryId([5;16]));
        let here=Parent::Root(a);let root=End::Ordinary(here);let to_left=End::Ordinary(left);let to_right=End::Ordinary(right);let bin=End::Recycle(r);
        let f=Member::File{entry:EntryId([6;16]),file:FileId([7;16])};
        let sibling=Member::File{entry:EntryId([8;16]),file:FileId([9;16])};
        let mut names=Names::default();names.enter(here,"report",f).unwrap();names.enter(right,"report",sibling).unwrap();
        names.apply_pair(root,to_left,"report",f).unwrap();assert_eq!(names.get(left,"report"),Some(f));
        names.apply_pair(to_left,root,"report",f).unwrap();assert_eq!(names.get(here,"report"),Some(f));
        names.apply_pair(root,to_left,"report",f).unwrap();names.apply_pair(to_left,bin,"report",f).unwrap();
        assert_eq!(names.get(left,"report"),None);assert!(!names.occupied_root(r,"report"));
        assert!(names.apply_pair(bin,to_right,"report",f).is_err());assert_eq!(names.get(right,"report"),Some(sibling));
        names.apply_pair(bin,root,"report",f).unwrap();assert!(!names.occupied_root(r,"report"));
        names.apply_pair(root,bin,"report",f).unwrap();names.apply_pair(bin,to_left,"report",f).unwrap();
        assert_eq!(names.get(left,"report"),Some(f));assert_eq!(names.get(right,"report"),Some(sibling));
        assert_eq!(names.get(Parent::Root(b),"report"),None);assert!(!names.occupied_root(r,"report"));
    }
    #[test]
    fn native_collision_in_one_child_cannot_withdraw_from_a_different_parent_on_failed_handoff_or_undo(){
        let a=VolumeId([1;16]);let b=VolumeId([2;16]);let id=FileId([3;16]);
        let file=Member::File{entry:EntryId([4;16]),file:id};
        let blocker=Member::Directory{entry:EntryId([5;16]),directory:DirectoryId([6;16])};
        let src=Parent::Child(a,DirectoryId([7;16]));let dst=Parent::Child(b,DirectoryId([8;16]));
        let display="Report.xlsx";let key=String::from_utf8(file.leaf(display).unwrap()).unwrap();
        let mut n=Names::default();n.enter(src,display,file).unwrap();n.enter(dst,&key,blocker).unwrap();
        assert_eq!(n.get(dst,display),None);assert!(n.apply_pair(End::Ordinary(src),End::Ordinary(dst),display,file).is_err());
        assert_eq!(n.get(src,display),Some(file));assert_eq!(n.get(dst,&key),Some(blocker));
        n.withdraw(dst,&key,blocker).unwrap();n.apply_pair(End::Ordinary(src),End::Ordinary(dst),display,file).unwrap();
        n.enter(src,&key,blocker).unwrap();
        assert!(n.apply_pair(End::Ordinary(dst),End::Ordinary(src),display,file).is_err());
        assert_eq!(n.get(dst,display),Some(file));assert_eq!(n.get(src,&key),Some(blocker));
    }
    #[test]
    fn exact_display_names_share_one_typed_namespace_without_case_or_unicode_aliases(){
        use crate::ordinary::storage_name::{DirectoryName,FileName};
        let v=VolumeId([1;16]);let p=Parent::Root(v);let e=EntryId([2;16]);let f=FileId([3;16]);
        let e2=EntryId([4;16]);let f2=FileId([5;16]);let dir=Member::Directory{entry:EntryId([6;16]),directory:DirectoryId([7;16])};
        let upper=FileName::parse(b"Report.xlsx").unwrap();let lower=FileName::parse(b"report.xlsx").unwrap();
        let mut n=Names::default();n.create_file(p,upper.display(),e,f).unwrap();n.create_file(p,lower.display(),e2,f2).unwrap();
        assert_eq!(n.get(p,"Report.xlsx"),Some(Member::File{entry:e,file:f}));assert_eq!(n.get(p,"report.xlsx"),Some(Member::File{entry:e2,file:f2}));
        assert_eq!(n.get(p,"REPORT.xlsx"),None);
        let same=DirectoryName::parse(b"Report.xlsx").unwrap();assert!(n.enter(p,same.display(),dir).is_err());
        let third=DirectoryName::parse(b"REPORT.xlsx").unwrap();n.enter(p,third.display(),dir).unwrap();assert_eq!(n.get(p,third.display()),Some(dir));
        n.hide_created(p,upper.display(),e,f).unwrap();assert_eq!(n.get(p,lower.display()),Some(Member::File{entry:e2,file:f2}));
        let same_directory=Member::Directory{entry:EntryId([8;16]),directory:DirectoryId([9;16])};
        n.enter(p,same.display(),same_directory).unwrap();assert!(n.create_file(p,upper.display(),e,f).is_err());
        n.withdraw(p,same.display(),same_directory).unwrap();n.create_file(p,upper.display(),e,f).unwrap();
        assert_eq!(n.get(p,upper.display()),Some(Member::File{entry:e,file:f}));
        let composed="Café";let decomposed="Cafe\u{301}";
        n.create_file(p,FileName::parse(composed.as_bytes()).unwrap().display(),EntryId([9;16]),FileId([10;16])).unwrap();
        assert_eq!(n.get(p,decomposed),None);
        let other_directory=Member::Directory{entry:EntryId([11;16]),directory:DirectoryId([12;16])};
        n.enter(p,DirectoryName::parse(decomposed.as_bytes()).unwrap().display(),other_directory).unwrap();
    }
}
