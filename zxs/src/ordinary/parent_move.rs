//! The flat FILE's true same-mount move between two ordinary parents.
//! No contents are copied. Move its original outer hard link first and
//! then its directory, without replacing any existing name. SQL must
//! already exclude both business names while a split shape can be observed.
use super::{Birth,Installed,RenameStage};
use crate::fs::{ComponentLimit,DIRECTORY_BYTES,DIRECTORY_NAMES,HeldBytes,HeldDir,
    NameBudget,WitnessRead,invalid,path_budget};
use std::{collections::BTreeSet,ffi::{CStr,CString},io,
    os::{fd::AsRawFd,raw::{c_char,c_int},unix::fs::FileExt}};

unsafe extern "C"{
    fn zxs_rename_fresh(olddir:c_int,old:*const c_char,newdir:c_int,new:*const c_char)->c_int;
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) enum ParentMoveStep{Old,LinkAtNew,New}
impl ParentMoveStep{
    fn body_moved(self)->bool{self==Self::New}
    fn link_moved(self)->bool{self!=Self::Old}
}
struct Sample{old:Vec<Vec<u8>>,new:Vec<Vec<u8>>,step:ParentMoveStep}
impl Sample{
    fn on(&self,new:bool)->&[Vec<u8>]{if new{&self.new}else{&self.old}}
}
struct Names{old:CString,new:CString,witness:CString}
impl Names{
    fn leaf(&self,new:bool)->&CStr{if new{&self.new}else{&self.old}}
    fn sample(&self,old:&HeldDir,new:&HeldDir)->io::Result<Sample>{
        let from=old.names()?;let to=new.names()?;
        let bo=old.exact_presence(&from,&self.old)?;
        let bn=new.exact_presence(&to,&self.new)?;
        let lo=old.exact_presence(&from,&self.witness)?;
        let ln=new.exact_presence(&to,&self.witness)?;
        let step=match (bo,bn,lo,ln){
            (true,false,true,false)=>ParentMoveStep::Old,
            (true,false,false,true)=>ParentMoveStep::LinkAtNew,
            (false,true,false,true)=>ParentMoveStep::New,
            (true,true,_,_)|(_,_,true,true)=>return Err(io::ErrorKind::AlreadyExists.into()),
            _=>return Err(invalid("FILE move has no exact original body and single outer link")),
        };
        Ok(Sample{old:from,new:to,step})
    }
    fn preflight(&self,to:&HeldDir,birth:&Birth,sample:&Sample)->io::Result<()>{
        let limit=ComponentLimit::held(to)?;
        let root=NameBudget::from_original(path_budget(&to.path)?,limit);
        root.child(&self.witness)?;
        let body=NameBudget::from_original(root.child(&self.new)?,limit);
        body.child(c"FILE")?;
        super::super::flat::local::require_resident_names(body,&birth.name)?;
        room(&sample.new,&[&self.witness,&self.new])
    }
}

pub(super) fn room(names:&[Vec<u8>],extra:&[&CStr])->io::Result<()>{
    let bytes=names.iter().map(|n|n.len()+1).sum::<usize>()+
        extra.iter().map(|n|n.to_bytes().len()+1).sum::<usize>();
    if names.len()+extra.len()>DIRECTORY_NAMES||bytes>DIRECTORY_BYTES{return Err(io::ErrorKind::StorageFull.into());}
    Ok(())
}
pub(super) fn require_parents(old:&HeldDir,new:&HeldDir)->io::Result<()>{
    if old.entity==new.entity||!old.same_mount(new)?{return Err(invalid("the actual FILE move parents are not distinct on one mount"));}
    WitnessRead::require_parent(old)?;WitnessRead::require_parent(new)
}
fn namespace(original:&HeldDir)->io::Result<HeldDir>{
    let held=HeldDir::trusted_storage(&original.path)?;
    held.exclusive()?;
    if held.entity!=original.entity||!held.same_mount(original)?{
        return Err(invalid("FILE move parent changed before the namespace lock"));
    }
    Ok(held)
}
pub(crate) fn lock_parents(old:&HeldDir,new:&HeldDir)->io::Result<(HeldDir,HeldDir)>{
    require_parents(old,new)?;
    // Both original namespace locks are acquired nonblocking in actual
    // file order. The caller already holds this one FILE directory EX.
    let (a,b)=if old.entity<new.entity{
        let a=namespace(old)?;let b=namespace(new)?;(a,b)
    }else{let b=namespace(new)?;let a=namespace(old)?;(a,b)};
    require_parents(&a,&b)?;Ok((a,b))
}
pub(super) fn rename_checked_names(from:&HeldDir,a:&CStr,to:&HeldDir,b:&CStr)->io::Result<()>{
    // SAFETY: exact still-held original parents and bounded components;
    // this accepts only the selected name checks of the local move holders.
    if unsafe{zxs_rename_fresh(from.file.as_raw_fd(),a.as_ptr(),to.file.as_raw_fd(),b.as_ptr())}<0{
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
pub(super) fn copy_names(sample:&[Vec<u8>])->io::Result<BTreeSet<Vec<u8>>>{
    let selected:BTreeSet<_>=sample.iter().cloned().collect();
    if selected.len()!=sample.len(){return Err(invalid("ambiguous FILE move parent census"));}
    Ok(selected)
}
fn side<'a>(old:&'a HeldDir,new:&'a HeldDir,at_new:bool)->&'a HeldDir{if at_new{new}else{old}}

pub(crate) struct FlatParentMove<'a>{
    birth:&'a Birth,old:&'a HeldDir,new:&'a HeldDir,names:Names,body:HeldDir,header:HeldBytes,step:ParentMoveStep,stage:RenameStage,
}
impl<'a> FlatParentMove<'a>{
    pub(crate) fn hold(birth:&'a Birth,old:&'a HeldDir,new:&'a HeldDir,old_name:&str,new_name:&str,
        stage:RenameStage,members:impl FnOnce(&WitnessRead<'_>)->io::Result<()>)->io::Result<Self>{
        require_parents(old,new)?;
        let names=Names{old:Birth::placed_container(old_name,birth.file)?,new:Birth::placed_container(new_name,birth.file)?,witness:birth.witness()?};
        let sample=names.sample(old,new)?;
        if stage==RenameStage::BeforeReserve{
            if sample.step!=ParentMoveStep::Old{return Err(invalid("a new FILE move must have both original names"));}
            names.preflight(new,birth,&sample)?;
        }
        let from=side(old,new,sample.step.body_moved());
        let body=from.directory_in_sample(names.leaf(sample.step.body_moved()),&copy_names(sample.on(sample.step.body_moved()))?)?
            .ok_or_else(||invalid("the original FILE body is absent"))?;
        if new.path.starts_with(&body.path)||old.path.starts_with(&body.path){
            return Err(invalid("a FILE move parent cannot be the FILE or its descendant"));
        }
        body.exclusive()?;
        members(&WitnessRead::at(&body)?)?;
        let header=Self::header(birth,&names,&body,old,new,&sample)?;
        if names.sample(old,new)?.step!=sample.step{return Err(invalid("original FILE moved sides during acquisition"));}
        Ok(Self{birth,old,new,names,body,header,step:sample.step,stage})
    }
    pub(crate) fn observed(&self)->ParentMoveStep{self.step}
    pub(crate) fn require_original(&self,opened:&Installed)->io::Result<()>{
        if self.step!=ParentMoveStep::Old||opened.placement_parent.is_some()||
            opened.witness!=self.names.witness||opened.dir.entity!=self.body.entity||
            !opened.dir.same_mount(&self.body)?||opened.header.stamp()!=self.header.stamp(){
            return Err(invalid("the move is not holding the original already-open FILE"));
        }
        opened.require(self.old)
    }
    pub(crate) fn require_reservable(&self,old:&HeldDir,new:&HeldDir)->io::Result<()>{
        if self.stage!=RenameStage::BeforeReserve||self.step!=ParentMoveStep::Old{
            return Err(invalid("not a fresh two-parent FILE reservation"));
        }
        let now=self.names.sample(old,new)?;
        if now.step!=ParentMoveStep::Old||
            Self::header(self.birth,&self.names,&self.body,old,new,&now)?.stamp()!=self.header.stamp(){
            return Err(invalid("the FILE or target names changed before reservation"));
        }
        room(&now.new,&[&self.names.witness,&self.names.new])
    }
    fn header(birth:&Birth,names:&Names,body:&HeldDir,old:&HeldDir,new:&HeldDir,sample:&Sample)->io::Result<HeldBytes>{
        let parent=side(old,new,sample.step.body_moved());
        let outer=side(old,new,sample.step.link_moved());
        let expected=birth.header();
        let got=WitnessRead::at(body)?.moved_file_header(parent,outer,
            &copy_names(sample.on(sample.step.link_moved()))?,&names.witness,expected.len() as u64)?;
        let mut buf=vec![0;expected.len()];got.file().read_exact_at(&mut buf,0)?;
        if buf!=expected{return Err(invalid("FILE move header differs from its original birth"));}
        got.unchanged()?;Ok(got)
    }
    /// The caller has already issued this exact operation. A cold OLD
    /// observation cannot start its first forward rename without the
    /// original live native owner, even if the ticket is still eligible.
    pub(crate) fn install<T>(self,admit:impl FnMut(ParentMoveStep)->io::Result<()>,
        point:impl FnMut(ParentMoveStep)->io::Result<()>,finish:impl FnOnce(&Installed)->io::Result<T>)->io::Result<(Installed,T)>{
        if self.stage==RenameStage::Reserved&&self.step==ParentMoveStep::Old{
            return Err(invalid("cold recovery cannot start an unexecuted FILE move"));
        }
        self.finish_at(ParentMoveStep::New,admit,point,finish)
    }
    pub(crate) fn restore<T>(self,admit:impl FnMut(ParentMoveStep)->io::Result<()>,
        point:impl FnMut(ParentMoveStep)->io::Result<()>,finish:impl FnOnce(&Installed)->io::Result<T>)->io::Result<(Installed,T)>{
        if self.stage!=RenameStage::Reserved{return Err(invalid("no reserved original FILE move to restore"));}
        self.finish_at(ParentMoveStep::Old,admit,point,finish)
    }
    fn check_step(&mut self,old:&HeldDir,new:&HeldDir,changed_link:bool)->io::Result<Sample>{
        let current=self.names.sample(old,new)?;
        if current.step!=self.step{return Err(invalid("the original FILE move phase changed"));}
        let next=Self::header(self.birth,&self.names,&self.body,old,new,&current)?;
        let last=self.header.stamp();
        let same=if changed_link{last.same_moved_file_header(next.stamp())}else{last==next.stamp()};
        if !same{return Err(invalid("FILE move did not retain the original two-name header inode"));}
        self.header=next;Ok(current)
    }
    fn sync_step(&mut self,old:&HeldDir,new:&HeldDir,changed_link:bool)->io::Result<()>{
        self.check_step(old,new,changed_link)?;
        new.file.sync_all()?;old.file.sync_all()?;
        self.check_step(old,new,false)?;Ok(())
    }
    fn finish_at<T>(mut self,goal:ParentMoveStep,mut admit:impl FnMut(ParentMoveStep)->io::Result<()>,
        mut point:impl FnMut(ParentMoveStep)->io::Result<()>,
        finish:impl FnOnce(&Installed)->io::Result<T>)->io::Result<(Installed,T)>{
        let (old,new)=lock_parents(self.old,self.new)?;
        // Even a cold, already-visible split or terminal state must be
        // synced on both parents before the next effect or SQL decision.
        self.sync_step(&old,&new,false)?;
        if self.step!=goal&&self.step!=ParentMoveStep::LinkAtNew{
            // For a whole transition the next side needs BOTH new names.
            // Do not strand an otherwise intact pair because an unrelated
            // entry filled the destination between preflight and these locks.
            let current=self.check_step(&old,&new,false)?;
            room(current.on(goal.body_moved()),&[&self.names.witness,self.names.leaf(goal.body_moved())])?;
        }
        while self.step!=goal{
            let next=match (self.step,goal){
                (ParentMoveStep::Old,ParentMoveStep::New)|(ParentMoveStep::New,ParentMoveStep::Old)=>ParentMoveStep::LinkAtNew,
                (ParentMoveStep::LinkAtNew,end)=>end,
                _=>return Err(invalid("not an original FILE move endpoint")),
            };
            let here=self.check_step(&old,&new,false)?;
            let link=self.step.link_moved()!=next.link_moved();
            let at_new=if link{self.step.link_moved()}else{self.step.body_moved()};
            let from=side(&old,&new,at_new);let to=side(&old,&new,!at_new);
            let (a,b)=if link{(self.names.witness.as_c_str(),self.names.witness.as_c_str())}
                else{(self.names.leaf(at_new),self.names.leaf(!at_new))};
            room(here.on(!at_new),&[b])?;
            let target=to.checked_child_path(b)?;
            admit(self.step)?;
            // Last admission is immediately before the no-replacement syscall.
            rename_checked_names(from,a,to,b)?;
            if !link{self.body.path=target;}
            self.step=next;point(next)?;
            self.sync_step(&old,&new,link)?;
        }
        let parent=side(&old,&new,self.step.body_moved());
        let value=Installed{dir:self.body,header:self.header,witness:self.names.witness,placement_parent:None};
        value.require(parent)?;
        let receipt=finish(&value)?;
        Ok((value,receipt))
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::{BranchId,EntryId,FileId,HistoryId,OperationId,PoolId,VolumeId,test_support::PrivateTemp};
    use std::{cell::Cell,fs::{self,OpenOptions},io::Write,os::unix::fs::{MetadataExt,OpenOptionsExt}};

    #[test]
    #[ignore="source only: actual flat FILE across two parents, no service"]
    fn the_single_original_file_survives_the_durable_split_without_copy(){
        let tmp=PrivateTemp::under(&std::env::temp_dir());
        let root=HeldDir::trusted_storage(&tmp.0).unwrap();
        let old=root.new_child_directory(c"Old").unwrap();
        let new=root.new_child_directory(c"New").unwrap();
        let birth=Birth{pool:PoolId([1;16]),volume:VolumeId([2;16]),operation:OperationId([3;16]),
            entry:EntryId([4;16]),file:FileId([5;16]),history:HistoryId([6;16]),branch:BranchId([7;16]),
            name:"Report".into(),ticket:OperationId([8;16]),slot:1};
        let source=old.new_child_directory(&birth.container().unwrap()).unwrap();
        let target=new.checked_child_path(&Birth::placed_container("report",birth.file).unwrap()).unwrap();
        let original_header=source.checked_child_path(c"FILE").unwrap();
        let source_link=old.checked_child_path(&birth.witness().unwrap()).unwrap();
        let target_link=new.checked_child_path(&birth.witness().unwrap()).unwrap();
        let write_header=|target:&std::path::Path|{
            let mut h=OpenOptions::new().create_new(true).write(true).mode(0o600).open(target).unwrap();
            h.write_all(&birth.header()).unwrap();h.sync_all().unwrap();
        };
        write_header(&original_header);fs::hard_link(&original_header,&source_link).unwrap();
        let inode=fs::metadata(&source_link).unwrap().ino();
        let hold=|mode|FlatParentMove::hold(&birth,&old,&new,"Report","report",mode,|p|p.only_header());
        let known=birth.working_records_at(&old,&old.names().unwrap().into_iter().collect(),false,|p|p.only_header()).unwrap();

        write_header(&target_link);
        assert!(matches!(hold(RenameStage::BeforeReserve),Err(e) if e.kind()==io::ErrorKind::AlreadyExists));
        assert!(source_link.exists()&&original_header.exists());fs::remove_file(&target_link).unwrap();
        let unchanged=hold(RenameStage::Reserved).unwrap();
        assert!(unchanged.install(|_|panic!("cold Old admission"),|_|panic!("cold Old rename"),|_|Ok(())).is_err());
        let cut=Cell::new(false);
        let first=hold(RenameStage::BeforeReserve).unwrap();
        first.require_original(&known).unwrap();
        assert!(first.install(|side|{assert_eq!(side,ParentMoveStep::Old);Ok(())},|side|{
            assert_eq!(side,ParentMoveStep::LinkAtNew);
            assert!(original_header.exists()&&target_link.exists()&&!source_link.exists()&&!target.exists());
            cut.set(true);Err(io::ErrorKind::Interrupted.into())
        },|_|->io::Result<()>{panic!("no move receipt before both names are synced")}).is_err());
        assert!(cut.get());assert!(known.require(&old).is_err());
        assert!(hold(RenameStage::BeforeReserve).is_err());

        let temporarily=new.checked_child_path(c"held-back").unwrap();
        fs::rename(&target_link,&temporarily).unwrap();
        write_header(&target_link);
        assert!(hold(RenameStage::Reserved).is_err());
        fs::remove_file(&target_link).unwrap();fs::rename(&temporarily,&target_link).unwrap();

        let split=hold(RenameStage::Reserved).unwrap();
        assert_eq!(split.observed(),ParentMoveStep::LinkAtNew);
        let (destination,())=split.install(|after|{assert_eq!(after,ParentMoveStep::LinkAtNew);Ok(())},
            |after|{assert_eq!(after,ParentMoveStep::New);Ok(())},|actual|{
                assert_eq!(actual.directory().entity,source.entity);
                assert_eq!(actual.directory().path,target);
                assert!(!source.path.exists());Ok(())
            }).unwrap();
        destination.require(&new).unwrap();
        assert_eq!(fs::metadata(target.join("FILE")).unwrap().ino(),inode);
        assert_eq!(fs::metadata(&target_link).unwrap().ino(),inode);
        drop(destination);

        let back=hold(RenameStage::Reserved).unwrap();assert_eq!(back.observed(),ParentMoveStep::New);
        let (prior,())=back.restore(|_|Ok(()),|_|Ok(()),|actual|{
            assert_eq!(actual.directory().entity,source.entity);
            assert!(source_link.exists()&&!target_link.exists()&&!target.exists());Ok(())
        }).unwrap();
        prior.require(&old).unwrap();
        assert_eq!(fs::metadata(&source_link).unwrap().ino(),inode);
    }

    #[test]
    fn immediately_usable_target_budget_bills_both_names(){
        let occupied=vec![b"x".to_vec();DIRECTORY_NAMES-1]; // count boundary only
        assert!(room(&occupied,&[c"leaf"]).is_ok());
        assert!(matches!(room(&occupied,&[c"leaf",c"witness"]),Err(e) if e.kind()==io::ErrorKind::StorageFull));
        let occupied=vec![vec![b'x';255];DIRECTORY_BYTES/256];
        assert!(matches!(room(&occupied,&[c"leaf"]),Err(e) if e.kind()==io::ErrorKind::StorageFull));
    }
}
