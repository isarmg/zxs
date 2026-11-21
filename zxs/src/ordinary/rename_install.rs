//! Same-ordinary-parent FILE rename. The original outer FILE witness never
//! moves. There is no copy, new container, or retirement of body materials.
//! The issuer supplies a confirmed birth, an exact containing parent, and
//! a new or previously reserved name edge; this module grants no authority.
use super::{Birth,Installed};
use crate::fs::{ComponentLimit,DIRECTORY_BYTES,HeldDir,NameBudget,WitnessRead,invalid,path_budget};
use std::{collections::BTreeSet,ffi::{CStr,CString},io,os::{fd::AsRawFd,raw::{c_char,c_int}}};

unsafe extern "C"{
    fn zxs_rename_fresh(olddir:c_int,old:*const c_char,newdir:c_int,new:*const c_char)->c_int;
}
#[derive(Clone,Copy,Eq,PartialEq)]
pub(crate) enum RenameStage{
    /// A fresh request may only select its actual old container.
    BeforeReserve,
    /// Only an unchanged exact SQL reservation can select the new name
    /// when the old name is absent. Never infer this state from disk.
    Reserved,
}
#[derive(Clone,Copy,Eq,PartialEq)]
enum End{Old,New}
struct Leaves{old:CString,new:CString}
impl Leaves{
    fn at(&self,end:End)->&CStr{match end{End::Old=>&self.old,End::New=>&self.new}}
    fn observe(&self,parent:&HeldDir,names:&[Vec<u8>],stage:RenameStage)->io::Result<End>{
        let old=parent.exact_presence(names,&self.old)?;
        let new=parent.exact_presence(names,&self.new)?;
        match (old,new,stage){
            (true,false,_)=>Ok(End::Old),
            (false,true,RenameStage::Reserved)=>Ok(End::New),
            (true,true,_)=>Err(io::ErrorKind::AlreadyExists.into()),
            _=>Err(invalid("neither exact reserved name can select the original FILE")),
        }
    }
    fn room(&self,names:&[Vec<u8>],from:End,to:End)->io::Result<()>{
        let used=names.iter().map(|name|name.len()+1).sum::<usize>();
        // This single rename replaces one native name, including its
        // length prefix; it never bills both names as simultaneous.
        let bytes=used.checked_sub(self.at(from).to_bytes().len()).and_then(|n|n.checked_add(self.at(to).to_bytes().len()))
            .filter(|n|*n<=DIRECTORY_BYTES);
        if bytes.is_none(){return Err(io::ErrorKind::StorageFull.into());}
        Ok(())
    }
}

pub(crate) struct RenameInstall<'a>{
    parent:&'a HeldDir,held:Installed,leaf:Leaves,end:End,stage:RenameStage,
}
impl<'a> RenameInstall<'a>{
    /// The actual containing parent can be the occupied ordinary directory
    /// or the existing private pocket of a prior physical transfer. The
    /// caller retains the enclosing placement and volume proofs.
    pub(crate) fn hold(birth:&Birth,parent:&'a HeldDir,old:&str,new:&str,stage:RenameStage,
        members:impl FnOnce(&WitnessRead<'_>)->io::Result<()>)->io::Result<Self>{
        let leaf=Leaves{old:Birth::placed_container(old,birth.file)?,new:Birth::placed_container(new,birth.file)?};
        if leaf.old==leaf.new{return Err(invalid("rename requires a different exact name"));}
        let native=ComponentLimit::held(parent)?;
        let parent_budget=NameBudget::from_original(path_budget(&parent.path)?,native);
        let finish=NameBudget::from_original(parent_budget.child(&leaf.new)?,native);
        finish.child(c"FILE")?;
        super::super::flat::local::require_resident_names(finish,&birth.name)?;
        let names=parent.names()?;
        let end=leaf.observe(parent,&names,stage)?;
        if stage==RenameStage::BeforeReserve{leaf.room(&names,End::Old,End::New)?;}
        let count=names.len();let names:BTreeSet<_>=names.into_iter().collect();
        if names.len()!=count{return Err(invalid("ambiguous containing parent enumeration"));}
        // The same FILE directory OFD excludes writers/transfer. The
        // unchanged issued outside witness authenticates either exact side.
        let held=birth.working_records_in(parent,&names,leaf.at(end),true,members)?;
        Ok(Self{parent,held,leaf,end,stage})
    }
    pub(crate) fn current_directory(&self)->&HeldDir{self.held.directory()}
    pub(crate) fn require_original(&self,opened:&Installed)->io::Result<()>{
        self.held.same_rename_body(opened)?;
        self.held.require(self.parent)
    }
    /// Only after the origin's actual reservation. A durable new name
    /// contains the exact same held object before the caller's final CAS.
    /// No parent lock is held during selection or work-material scans.
    pub(crate) fn install<T>(self,admit:impl FnOnce()->io::Result<()>,
        after_rename:impl FnOnce()->io::Result<()>,
        finish:impl FnOnce(&Installed)->io::Result<T>)->io::Result<(Installed,T)>{
        self.transition(End::New,admit,after_rename,finish)
    }
    pub(crate) fn already_renamed(&self)->bool{self.end==End::New}
    /// Cold recovery can durably prove the observed original side, not
    /// start a forward rename without the live native open's admission.
    pub(crate) fn finish_observed<T>(self,finish:impl FnOnce(&Installed)->io::Result<T>)
        ->io::Result<(Installed,T)>{
        if self.stage!=RenameStage::Reserved{return Err(invalid("no original rename reservation to settle"));}
        let observed=self.end;
        self.transition(observed,||Err(invalid("cold recovery cannot change the observed name")),||Ok(()),finish)
    }
    /// A still-unselected original may give up its reservation only after
    /// its old name is durable again. Do not erase either unknown side.
    pub(crate) fn restore_old<T>(self,admit:impl FnOnce()->io::Result<()>,
        finish:impl FnOnce(&Installed)->io::Result<T>)->io::Result<(Installed,T)>{
        if self.stage!=RenameStage::Reserved{return Err(invalid("no original rename reservation to abandon"));}
        self.transition(End::Old,admit,||Ok(()),finish)
    }
    fn transition<T>(mut self,goal:End,admit:impl FnOnce()->io::Result<()>,
        after_rename:impl FnOnce()->io::Result<()>,
        finish:impl FnOnce(&Installed)->io::Result<T>)->io::Result<(Installed,T)>{
        let parent=HeldDir::trusted_storage(&self.parent.path)?;
        parent.exclusive()?;
        if parent.entity!=self.parent.entity||!parent.same_mount(self.parent)?{
            return Err(invalid("the containing parent changed before original rename"));
        }
        let before=parent.names()?;
        if self.leaf.observe(&parent,&before,RenameStage::Reserved)?!=self.end{
            return Err(invalid("the original FILE changed selected sides"));
        }
        self.held.require(&parent)?;
        if self.end!=goal{
            let from=self.leaf.at(self.end);let to=self.leaf.at(goal);
            self.leaf.room(&before,self.end,goal)?;
            let target=parent.checked_child_path(to)?;
            admit()?;
            // SAFETY: bounded local names and the exact still-held parent.
            // The kernel will not replace a file, directory, or symlink.
            let fd=parent.file.as_raw_fd();
            if unsafe{zxs_rename_fresh(fd,from.as_ptr(),fd,to.as_ptr())}<0{
                return Err(io::Error::last_os_error());
            }
            self.held.dir.path=target;
            after_rename()?;
        }
        // A cold retry cannot assume the formerly visible name is durable.
        if self.leaf.observe(&parent,&parent.names()?,RenameStage::Reserved)?!=goal{
            return Err(invalid("the original FILE is not at its sole intended name"));
        }
        self.held.require(&parent)?;
        parent.file.sync_all()?;
        if self.leaf.observe(&parent,&parent.names()?,RenameStage::Reserved)?!=goal{
            return Err(invalid("the installed original FILE changed after sync"));
        }
        self.held.require(&parent)?;
        let receipt=finish(&self.held)?;
        Ok((self.held,receipt))
    }
}

impl Installed{
    pub(crate) fn containing_parent<'a>(&'a self,root:&'a HeldDir)->io::Result<&'a HeldDir>{
        self.require(root)?;Ok(self.placement_parent.as_ref().unwrap_or(root))
    }
    fn same_rename_body(&self,other:&Installed)->io::Result<()>{
        if self.witness!=other.witness||self.dir.entity!=other.dir.entity||!self.dir.same_mount(&other.dir)?
            ||self.header.stamp()!=other.header.stamp(){
            return Err(invalid("renamed FILE differs from the originally opened object"));
        }
        Ok(())
    }
    /// After the caller has independently selected its committed position,
    /// the issuing reader can retain its original body and header FDs.
    /// Never replace an old handle with a new inode, witness or mount.
    pub(crate) fn rebind_renamed(&mut self,root:&HeldDir,target:&Installed)->io::Result<()>{
        let parent=self.placement_parent.as_ref().unwrap_or(root);
        if target.placement_parent.is_some(){return Err(invalid("renamed body proof must identify its immediate parent"));}
        self.same_rename_body(target)?;
        target.require(parent)?;
        let old=std::mem::replace(&mut self.dir.path,target.dir.path.clone());
        if let Err(error)=self.require(root){self.dir.path=old;return Err(error);}
        Ok(())
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::{BranchId,EntryId,FileId,HistoryId,OperationId,PoolId,VolumeId,test_support::PrivateTemp};
    use std::{fs::{self,OpenOptions},io::Write,os::unix::fs::{DirBuilderExt,MetadataExt,OpenOptionsExt}};
    #[test]
    #[ignore="source only: same original directory and physical witness; no service"]
    fn exact_rename_reuses_the_file_and_witness_after_the_true_pre_sync_cut(){
        let tmp=PrivateTemp::under(&std::env::temp_dir());
        let parent=HeldDir::trusted_storage(&tmp.0).unwrap();
        let birth=Birth{pool:PoolId([1;16]),volume:VolumeId([2;16]),operation:OperationId([3;16]),
            entry:EntryId([4;16]),file:FileId([5;16]),history:HistoryId([6;16]),branch:BranchId([7;16]),
            name:"Report.txt".into(),ticket:OperationId([8;16]),slot:1};
        let old=birth.container().unwrap();let new=Birth::placed_container("report.txt",birth.file).unwrap();
        let d=parent.new_child_directory(&old).unwrap();
        let h=d.checked_child_path(c"FILE").unwrap();
        let mut file=OpenOptions::new().create_new(true).write(true).mode(0o600).open(&h).unwrap();
        file.write_all(&birth.header()).unwrap();file.sync_all().unwrap();
        let w=parent.checked_child_path(&birth.witness().unwrap()).unwrap();
        fs::hard_link(&h,&w).unwrap();
        let original=d.entity;let witness=fs::metadata(&w).unwrap().ino();
        let target=parent.checked_child_path(&new).unwrap();
        fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
        let select=|s|RenameInstall::hold(&birth,&parent,"Report.txt","report.txt",s,|p|p.only_header());
        assert!(matches!(select(RenameStage::BeforeReserve),Err(e) if e.kind()==io::ErrorKind::AlreadyExists));
        assert!(h.exists());fs::remove_dir(&target).unwrap();
        let mut reader=birth.working_records_at(&parent,&parent.names().unwrap().into_iter().collect(),false,
            |p|p.only_header()).unwrap();
        let change=select(RenameStage::BeforeReserve).unwrap();
        assert_eq!(change.current_directory().entity,original);
        assert!(matches!(select(RenameStage::BeforeReserve),Err(e) if e.kind()==io::ErrorKind::WouldBlock));
        let cut=std::cell::Cell::new(false);
        assert!(change.install(||Ok(()),||{
            assert!(!d.path.exists()&&target.is_dir());cut.set(true);Err(io::ErrorKind::Interrupted.into())
        },|_|->io::Result<()>{panic!("parent sync and final assertion not reached")}).is_err());
        assert!(cut.get());assert!(select(RenameStage::BeforeReserve).is_err());
        let (installed,())=select(RenameStage::Reserved).unwrap().install(
            ||panic!("cold retry must not execute new rename"),
            ||panic!("cold retry must not move anything"),
            |actual|{assert_eq!(actual.directory().entity,original);assert_eq!(actual.directory().path,target);Ok(())}
        ).unwrap();
        installed.require(&parent).unwrap();
        assert!(reader.require(&parent).is_err());
        reader.rebind_renamed(&parent,&installed).unwrap();
        reader.require(&parent).unwrap();
        assert_eq!(fs::metadata(target.join("FILE")).unwrap().ino(),witness);
        assert_eq!(fs::metadata(w).unwrap().ino(),witness);
        assert!(!d.path.exists());
        drop(installed);
        // This tests physical rollback only; a committed SQL operation
        // cannot supply RenameStage::Reserved to the eventual issuer.
        let (back,())=select(RenameStage::Reserved).unwrap().restore_old(||Ok(()),|actual|{
            assert_eq!(actual.directory().entity,original);
            assert_eq!(actual.directory().path,d.path);
            assert!(!target.exists());Ok(())
        }).unwrap();
        reader.rebind_renamed(&parent,&back).unwrap();
        assert_eq!(fs::metadata(&h).unwrap().ino(),witness);
    }
}
