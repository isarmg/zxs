//! A FILE already placed inside its original transfer pocket moves the
//! whole pocket, not its contents. Its held FILE header, outside link,
//! body and pocket inode are unchanged, including for an old R retirement
//! guard. The caller owns the exact original SQL reservation and both
//! ordinary paths; these physical observations do not authorize a move.
use super::{Birth,Installed,RenameStage,parent_move::{copy_names,lock_parents,rename_checked_names,require_parents,room}};
use crate::{OperationId,fs::{HeldDir,WitnessRead,invalid}};
use std::{ffi::{CStr,CString},io};

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) enum PackedParentStep{Old,PocketAtNew,New}
impl PackedParentStep{
    fn target(self)->bool{self!=Self::Old}
}
struct Names{storage:CString,old:CString,new:CString,witness:CString}
struct Observed{pocket:HeldDir,names:Vec<Vec<u8>>,step:PackedParentStep}
impl Names{
    fn terminal(&self,side:PackedParentStep)->&CStr{if side==PackedParentStep::New{&self.new}else{&self.old}}
    fn observe(&self,old:&HeldDir,new:&HeldDir)->io::Result<Observed>{
        let a=old.names()?;let b=new.names()?;
        let target=match (old.exact_presence(&a,&self.storage)?,new.exact_presence(&b,&self.storage)?){
            (true,false)=>false,(false,true)=>true,
            (true,true)=>return Err(io::ErrorKind::AlreadyExists.into()),
            _=>return Err(invalid("original FILE pocket is absent from both reserved parents")),
        };
        let (parent,sample)=if target{(new,b)}else{(old,a)};
        let pocket=parent.directory_in_sample(&self.storage,&copy_names(&sample)?)?
            .ok_or_else(||invalid("missing exact original FILE pocket"))?;
        let names=pocket.names()?;
        if names.len()!=2||!pocket.exact_presence(&names,&self.witness)?{
            return Err(invalid("original FILE pocket must contain only its body and paired witness"));
        }
        let old_name=pocket.exact_presence(&names,&self.old)?;
        let new_name=self.old!=self.new&&pocket.exact_presence(&names,&self.new)?;
        let step=match (target,old_name,new_name,self.old==self.new){
            (false,true,false,_)=>PackedParentStep::Old,
            (true,true,false,true)|(true,false,true,false)=>PackedParentStep::New,
            (true,true,false,false)=>PackedParentStep::PocketAtNew,
            _=>return Err(invalid("unexpected spelling or FILE link in the original pocket")),
        };
        Ok(Observed{pocket,names,step})
    }
}

pub(crate) struct PackedParentMove<'a>{
    old:&'a HeldDir,new:&'a HeldDir,names:Names,file:Installed,step:PackedParentStep,stage:RenameStage,
}
impl<'a> PackedParentMove<'a>{
    pub(crate) fn hold(birth:&Birth,backing:OperationId,old:&'a HeldDir,new:&'a HeldDir,old_name:&str,new_name:&str,
        stage:RenameStage,members:impl FnOnce(&WitnessRead<'_>)->io::Result<()>)->io::Result<Self>{
        require_parents(old,new)?;
        let names=Names{storage:Birth::placement_parent(backing)?,
            old:Birth::placed_container(old_name,birth.file)?,new:Birth::placed_container(new_name,birth.file)?,
            witness:birth.witness()?};
        let state=names.observe(old,new)?;
        if stage==RenameStage::BeforeReserve{
            if state.step!=PackedParentStep::Old{return Err(invalid("fresh FILE move needs its original pocket"));}
            let limit=crate::fs::ComponentLimit::held(new)?;
            // If the display also changes the new pocket can hold the old
            // name for one phase. Both real path geometries must fit.
            birth.validate_transfer_geometry(old_name,(&new.path,limit),backing)?;
            birth.validate_transfer_geometry(new_name,(&new.path,limit),backing)?;
            room(&new.names()?,&[&names.storage])?;
        }
        let parent=if state.step.target(){new}else{old};
        let file=birth.working_records_in(&state.pocket,&copy_names(&state.names)?,
            names.terminal(state.step),true,members)?.with_placement_parent(parent,state.pocket)?;
        let installed=file.placement_parent.as_ref().ok_or_else(||invalid("FILE move has no original pocket"))?;
        if new.path.starts_with(&installed.path)||old.path.starts_with(&installed.path){
            return Err(invalid("a FILE move parent cannot be within the original pocket"));
        }
        let hold=Self{old,new,names,file,step:state.step,stage};
        hold.check(old,new)?;Ok(hold)
    }
    pub(crate) fn observed(&self)->PackedParentStep{self.step}
    pub(crate) fn require_reservable(&self,old:&HeldDir,new:&HeldDir)->io::Result<()>{
        if self.stage!=RenameStage::BeforeReserve||self.step!=PackedParentStep::Old{
            return Err(invalid("not a fresh two-parent pocket reservation"));
        }
        self.check(old,new)?;
        room(&new.names()?,&[&self.names.storage])
    }
    pub(crate) fn require_original(&self,opened:&Installed)->io::Result<()>{
        let a=self.pocket()?;
        let b=opened.placement_parent.as_ref().ok_or_else(||invalid("open FILE has no original pocket"))?;
        if self.step!=PackedParentStep::Old||a.entity!=b.entity||!a.same_mount(b)?||
            self.file.dir.entity!=opened.dir.entity||!self.file.dir.same_mount(&opened.dir)?||
            self.file.header.stamp()!=opened.header.stamp()||self.file.witness!=opened.witness{
            return Err(invalid("move does not hold the originally open pocket and FILE"));
        }
        opened.require(self.old)
    }
    fn pocket(&self)->io::Result<&HeldDir>{
        self.file.placement_parent.as_ref().ok_or_else(||invalid("original FILE move lost its pocket"))
    }
    fn check(&self,old:&HeldDir,new:&HeldDir)->io::Result<()>{
        let state=self.names.observe(old,new)?;
        let p=self.pocket()?;
        if state.step!=self.step||state.pocket.entity!=p.entity||!state.pocket.same_mount(p)?||state.pocket.path!=p.path||
            self.file.dir.path!=p.checked_child_path(self.names.terminal(self.step))?{
            return Err(invalid("the original FILE pocket, child or phase changed"));
        }
        self.file.require(if self.step.target(){new}else{old})
    }
    fn sync(&self,old:&HeldDir,new:&HeldDir)->io::Result<()>{
        self.check(old,new)?;
        self.pocket()?.file.sync_all()?;
        new.file.sync_all()?;old.file.sync_all()?;
        self.check(old,new)
    }
    pub(crate) fn install<T>(self,admit:impl FnMut(PackedParentStep)->io::Result<()>,
        point:impl FnMut(PackedParentStep)->io::Result<()>,finish:impl FnOnce(&Installed)->io::Result<T>)->io::Result<(Installed,T)>{
        if self.stage==RenameStage::Reserved&&self.step==PackedParentStep::Old{
            return Err(invalid("cold recovery cannot start an unexecuted pocket move"));
        }
        self.transition(PackedParentStep::New,admit,point,finish)
    }
    pub(crate) fn restore<T>(self,admit:impl FnMut(PackedParentStep)->io::Result<()>,
        point:impl FnMut(PackedParentStep)->io::Result<()>,finish:impl FnOnce(&Installed)->io::Result<T>)->io::Result<(Installed,T)>{
        if self.stage!=RenameStage::Reserved{return Err(invalid("no original pocket move to restore"));}
        self.transition(PackedParentStep::Old,admit,point,finish)
    }
    fn transition<T>(mut self,goal:PackedParentStep,mut admit:impl FnMut(PackedParentStep)->io::Result<()>,
        mut point:impl FnMut(PackedParentStep)->io::Result<()>,
        finish:impl FnOnce(&Installed)->io::Result<T>)->io::Result<(Installed,T)>{
        let (old,new)=lock_parents(self.old,self.new)?;
        self.sync(&old,&new)?;
        while self.step!=goal{
            let different=self.names.old!=self.names.new;
            let next=match (self.step,goal,different){
                (PackedParentStep::Old,PackedParentStep::New,true)|(PackedParentStep::New,PackedParentStep::Old,true)=>PackedParentStep::PocketAtNew,
                (PackedParentStep::Old,PackedParentStep::New,false)|(PackedParentStep::New,PackedParentStep::Old,false)|
                (PackedParentStep::PocketAtNew,_,_)=>goal,
                _=>return Err(invalid("invalid original pocket move successor")),
            };
            self.check(&old,&new)?;
            let p=self.pocket()?;
            if self.step.target()!=next.target(){
                let (from,to)=if self.step.target(){(&new,&old)}else{(&old,&new)};
                room(&to.names()?,&[&self.names.storage])?;
                let path=to.checked_child_path(&self.names.storage)?;
                // Check the absolute next-body path before touching the pocket.
                let file=path.join(self.names.terminal(next).to_str().map_err(|_|invalid("non-UTF-8 FILE terminal"))?);
                crate::fs::path_budget(&file)?;
                admit(self.step)?;
                rename_checked_names(from,&self.names.storage,to,&self.names.storage)?;
                self.file.placement_parent.as_mut().ok_or_else(||invalid("lost original FILE pocket"))?.path=path;
                self.file.dir.path=file;
            }else{
                let a=self.names.terminal(self.step);let b=self.names.terminal(next);
                let path=p.checked_child_path(b)?;
                admit(self.step)?;
                rename_checked_names(p,a,p,b)?;
                self.file.dir.path=path;
            }
            self.step=next;point(next)?;
            self.sync(&old,&new)?;
        }
        let result=finish(&self.file)?;
        Ok((self.file,result))
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::{BranchId,EntryId,FileId,HistoryId,PoolId,VolumeId,test_support::PrivateTemp};
    use std::{cell::Cell,fs::{self,OpenOptions},io::Write,os::unix::fs::OpenOptionsExt};
    #[test]
    #[ignore="source only: real transfer pocket and old retained source guard; no service"]
    fn original_pocket_and_retirement_target_survive_both_parent_changes(){
        let tmp=PrivateTemp::under(&std::env::temp_dir());
        let root=HeldDir::trusted_storage(&tmp.0).unwrap();
        let old=root.new_child_directory(c"Old").unwrap();
        let new=root.new_child_directory(c"New").unwrap();
        let birth=Birth{pool:PoolId([1;16]),volume:VolumeId([2;16]),operation:OperationId([3;16]),
            entry:EntryId([4;16]),file:FileId([5;16]),history:HistoryId([6;16]),branch:BranchId([7;16]),
            name:"Report".into(),ticket:OperationId([8;16]),slot:1};
        let after_restore=OperationId([9;16]);let name=Birth::placement_parent(after_restore).unwrap();
        let pocket=old.new_child_directory(&name).unwrap();
        let moved=new.checked_child_path(&name).unwrap();
        let terminal=birth.container().unwrap();
        let body=pocket.new_child_directory(&terminal).unwrap();
        let head=body.path.join("FILE");
        let mut f=OpenOptions::new().create_new(true).write(true).mode(0o600).open(&head).unwrap();
        f.write_all(&birth.header()).unwrap();f.sync_all().unwrap();
        fs::hard_link(&head,pocket.checked_child_path(&birth.witness().unwrap()).unwrap()).unwrap();
        let opened=birth.working_records_in(&pocket,&pocket.names().unwrap().into_iter().collect(),&terminal,false,
            |w|w.only_header()).unwrap().with_placement_parent(&old,old.exact_directory(&name).unwrap().unwrap()).unwrap();
        let proof=opened.capture_retirement(&old).unwrap();
        let hold=|from,to,state|PackedParentMove::hold(&birth,after_restore,&old,&new,from,to,state,|w|w.only_header());

        let move_one=hold("Report","report",RenameStage::BeforeReserve).unwrap();
        move_one.require_original(&opened).unwrap();
        let cut=Cell::new(false);
        assert!(move_one.install(|s|{assert_eq!(s,PackedParentStep::Old);Ok(())},|s|{
            assert_eq!(s,PackedParentStep::PocketAtNew);assert!(!pocket.path.exists());
            assert!(moved.join(terminal.to_str().unwrap()).join("FILE").exists());
            cut.set(true);Err(io::ErrorKind::Interrupted.into())
        },|_|->io::Result<()>{panic!("no receipt for an intermediate pocket")}).is_err());
        assert!(cut.get());assert!(opened.require(&old).is_err());

        let resumed=hold("Report","report",RenameStage::Reserved).unwrap();
        assert_eq!(resumed.observed(),PackedParentStep::PocketAtNew);
        let (there,())=resumed.install(|s|{assert_eq!(s,PackedParentStep::PocketAtNew);Ok(())},
            |s|{assert_eq!(s,PackedParentStep::New);Ok(())},|one|{
                assert_eq!(one.dir.entity,body.entity);
                assert_eq!(one.placement_parent.as_ref().unwrap().entity,pocket.entity);
                one.still_retirement_target(&new,&proof)
            }).unwrap();
        assert!(there.header.stamp()==opened.header.stamp());
        drop(there);

        let trace=std::cell::RefCell::new(Vec::new());
        let (prior,())=hold("Report","report",RenameStage::Reserved).unwrap()
            .restore(|_|Ok(()),|s|{trace.borrow_mut().push(s);Ok(())},|one|
                one.still_retirement_target(&old,&proof)
            ).unwrap();
        assert_eq!(*trace.borrow(),[PackedParentStep::PocketAtNew,PackedParentStep::Old]);
        drop(prior);
        let trace=std::cell::RefCell::new(Vec::new());
        let (same_name,())=hold("Report","Report",RenameStage::BeforeReserve).unwrap()
            .install(|_|Ok(()),|s|{trace.borrow_mut().push(s);Ok(())},|one|
                one.still_retirement_target(&new,&proof)
            ).unwrap();
        assert_eq!(*trace.borrow(),[PackedParentStep::New]);
        same_name.require(&new).unwrap();
    }
}
