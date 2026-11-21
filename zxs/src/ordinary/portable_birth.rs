//! An original portable FILE birth describes an exact container/role set.
//! It is not a grant: only the same admitted SYSTEM/share and its registered
//! ordinary parent can use this for a listing or a later I/O selection.
use super::storage_name::{FileName,ResidentNames,TicketLeaf};
use crate::{BranchId,EntryId,FileId,HistoryId,OperationId,PoolId,VolumeId,
    fs::{ComponentLimit,HeldBytes,HeldDir,NameBudget,WitnessRead,invalid,path_budget}};
use std::{collections::BTreeSet,ffi::{CStr,CString},io,os::unix::fs::FileExt,path::Path};

#[derive(Clone,Debug,Eq,PartialEq)]
pub(crate) struct Birth {
    pub(crate) pool:PoolId,pub(crate) volume:VolumeId,pub(crate) operation:OperationId,
    pub(crate) entry:EntryId,pub(crate) file:FileId,pub(crate) history:HistoryId,pub(crate) branch:BranchId,
    pub(crate) name:String,
    pub(crate) ticket:OperationId,pub(crate) slot:u8,
}
impl Birth{
    pub(crate) fn reserved_spelling(name:&[u8])->bool{TicketLeaf::parse(name).is_some()}
    pub(crate) fn reservation_leaves(op:OperationId,ticket:OperationId)->io::Result<(CString,CString)>{
        Ok((TicketLeaf::stage(op,7)?.name(),TicketLeaf::stage(ticket,0)?.name()))
    }
    pub(crate) fn validate_geometry(&self,ordinary:(&Path,ComponentLimit),recycle:(&Path,ComponentLimit),mark:&CStr)->io::Result<()>{
        let words=ResidentNames::for_file(FileName::parse(self.name.as_bytes())?,self.file,self.ticket)?;
        if self.operation==self.ticket||TicketLeaf::stage(self.ticket,self.slot)?.name().as_c_str()==mark{
            return Err(invalid("FILE operation, stage or root marker overlap"));}
        // Check the known interior role names as well as the outer terminal
        // and private slots in BOTH registered geometries. No R is opened.
        for (path,limit) in [ordinary,recycle]{
            let parent=NameBudget::from_original(path_budget(path)?,limit);
            let end=words.at(parent)?;let isolation=TicketLeaf::isolation(self.ticket)?.name();
            let widest=if words.terminal().name().to_bytes().len()>=isolation.to_bytes().len(){end}else{parent.child(&isolation)?};
            let container=NameBudget::from_original(widest,limit);
            container.child(c"FILE")?;
            super::flat::local::require_resident_names(container,&self.name)?;
        }
        Ok(())
    }
    // The OS I/O is relative to proved held parents; this additionally
    // checks the existing bounded name and reopen budgets before new work.
    // Do not apply it retroactively to immutable birth/receipt queries.
    pub(crate) fn validate_transfer_geometry(&self,display:&str,target:(&Path,ComponentLimit),action:OperationId)->io::Result<()>{
        let (path,limit)=target;
        let root=NameBudget::from_original(path_budget(path)?,limit);
        let leaf=Self::placement_parent(action)?;
        let cavity=NameBudget::from_original(root.child(&leaf)?,limit);
        let terminal=Self::placed_container(display,self.file)?;
        cavity.child(&self.witness()?)?;
        let container=NameBudget::from_original(cavity.child(&terminal)?,limit);
        container.child(c"FILE")?;
        super::flat::local::require_resident_names(container,&self.name)
    }
    pub(crate) fn container(&self)->io::Result<CString>{Self::placed_container(&self.name,self.file)}
    pub(crate) fn placed_container(display:&str,file:FileId)->io::Result<CString>{
        super::storage_name::file_leaf(display,file)
    }
    pub(crate) fn placement_parent(op:OperationId)->io::Result<CString>{
        if op.0==[0xff;16]{return Err(invalid("transfer parent cannot be a reserved volume role"));}
        Ok(TicketLeaf::stage(op,5)?.name())
    }
    pub(crate) fn retirement_name(op:OperationId)->io::Result<CString>{Ok(TicketLeaf::stage(op,6)?.name())}
    pub(crate) fn witness(&self)->io::Result<CString>{Ok(TicketLeaf::stage(self.operation,7)?.name())}
    pub(crate) fn lower(&self,i:u8)->io::Result<CString>{Ok(TicketLeaf::stage(self.ticket,i)?.name())}
    pub(crate) fn header(&self)->Vec<u8>{
        // Immutable across subsequent physical placements. The native pair
        // (not these bytes or an inode number) can detect a one-sided copy.
        let mut b=b"ZXS-PORTABLE-1/FILE\0".to_vec();
        for id in [self.pool.0,self.operation.0,self.entry.0,self.file.0,self.history.0,self.branch.0]{b.extend(id);}
        b.push(self.name.len() as u8);b.extend(self.name.as_bytes());b
    }
    pub(crate) fn installed(&self,parent:&HeldDir,sample:&BTreeSet<Vec<u8>>)->io::Result<Installed>{
        self.newborn_at(parent,sample,&self.container()?)
    }
    fn newborn_at(&self,parent:&HeldDir,sample:&BTreeSet<Vec<u8>>,leaf:&CStr)->io::Result<Installed>{
        self.physical_at(parent,sample,leaf,HeldDir::share,|scope|scope.only_header())
    }
    pub(crate) fn working_at(&self,parent:&HeldDir,sample:&BTreeSet<Vec<u8>>,write:bool,
        spec:Option<(crate::ordinary::working_sql::Spec,bool)>,kept:&crate::ordinary::working_sql::Retained)->io::Result<Installed>{
        // Same native directory EX serializes preparers of this one first
        // revision. It is not a network-handle lock. A selected read holds
        // the original header and its immutable materials, never this lock.
        self.physical_at(parent,sample,&self.container()?,|dir|if write{dir.exclusive()}else{Ok(())},|scope|match spec {
            None=>scope.only_header(),
            Some((s,all)) if s.pool==self.pool && s.file==self.file=>s.members(scope,all,kept),
            _=>Err(invalid("working materials belong to a different FILE")),
        })
    }
    pub(crate) fn working_records_at(&self,parent:&HeldDir,sample:&BTreeSet<Vec<u8>>,write:bool,
        members:impl FnOnce(&WitnessRead<'_>)->io::Result<()>)->io::Result<Installed>{
        self.working_records_in(parent,sample,&self.container()?,write,members)
    }
    pub(crate) fn working_records_in(&self,parent:&HeldDir,sample:&BTreeSet<Vec<u8>>,leaf:&CStr,write:bool,
        members:impl FnOnce(&WitnessRead<'_>)->io::Result<()>)->io::Result<Installed>{
        self.physical_at(parent,sample,leaf,|dir|if write{dir.exclusive()}else{Ok(())},members)
    }
    fn physical_at(&self,parent:&HeldDir,sample:&BTreeSet<Vec<u8>>,leaf:&CStr,lock:impl FnOnce(&HeldDir)->io::Result<()>,
        members:impl FnOnce(&WitnessRead<'_>)->io::Result<()>)->io::Result<Installed>{
        let dir=parent.directory_in_sample(leaf,sample)?.ok_or_else(||invalid("original FILE container not installed"))?;
        lock(&dir)?;
        let scope=WitnessRead::at(&dir)?;members(&scope)?;
        let expected=self.header();
        let witness=self.witness()?;
        let header=scope.file_header(parent,sample,&witness,expected.len() as u64)?;
        let mut bytes=vec![0;expected.len()];header.file().read_exact_at(&mut bytes,0)?;
        if bytes!=expected{return Err(invalid("FILE header differs from original creation identity"));}
        let result=Installed{dir,header,witness,placement_parent:None};result.require(parent)?;Ok(result)
    }
}
pub(crate) struct Installed{dir:HeldDir,header:HeldBytes,witness:CString,placement_parent:Option<HeldDir>}
impl Installed{
    pub(crate) fn duplicate(&self)->io::Result<Self>{
        Ok(Self{dir:self.dir.duplicate()?,header:self.header.duplicate()?,witness:self.witness.clone(),
            placement_parent:self.placement_parent.as_ref().map(HeldDir::duplicate).transpose()?})
    }
    pub(crate) fn directory(&self)->&HeldDir{&self.dir}
    /// The independently locked source of a copy must still be the
    /// already-open FILE, not a newly coherent object at its old spelling.
    pub(crate) fn require_copy_source(&self,root:&HeldDir,locked:&Self)->io::Result<()>{
        self.require(root)?;locked.require(root)?;
        if self.dir.entity!=locked.dir.entity||self.dir.path!=locked.dir.path||!self.dir.same_mount(&locked.dir)?||
           self.witness!=locked.witness||self.header.stamp()!=locked.header.stamp(){
            return Err(invalid("handoff source no longer matches the opened FILE"));
        }
        match (&self.placement_parent,&locked.placement_parent){
            (None,None)=>Ok(()),
            (Some(a),Some(b)) if a.path==b.path&&a.entity==b.entity&&a.same_mount(b)?=>Ok(()),
            _=>Err(invalid("handoff source no longer matches its opened pocket")),
        }
    }
    pub(crate) fn capture_retirement(&self,root:&HeldDir)->io::Result<crate::fs::RetireProof>{
        self.require(root)?;
        crate::fs::RetireProof::capture(self.placement_parent.as_ref().unwrap_or(root),&self.dir,&self.header,&self.witness)
    }
    pub(crate) fn verified_retirement_target(&self,root:&HeldDir,used:&crate::ordinary::working_round::Used)->io::Result<crate::fs::RetireProof>{
        self.require(root)?;
        crate::fs::RetireProof::from_verified(self.placement_parent.as_ref().unwrap_or(root),&self.dir,&self.header,&self.witness,used)
    }
    pub(crate) fn still_retirement_target(&self,root:&HeldDir,p:&crate::fs::RetireProof)->io::Result<()>{
        self.require(root)?;
        p.requires_target(self.placement_parent.as_ref().unwrap_or(root),&self.dir,&self.witness)
    }
    /// Consume the independently verified endpoint of the exact completed
    /// same-mount move. Keep this reader's original body FD. Only the
    /// flat outer hard-link rename can legitimately update header ctime.
    pub(crate) fn rebind_moved(&mut self,new_root:&HeldDir,moved:Self)->io::Result<()>{
        moved.require(new_root)?;
        if self.witness!=moved.witness||self.dir.entity!=moved.dir.entity||!self.dir.same_mount(&moved.dir)?{
            return Err(invalid("moved FILE differs from the originally opened body"));
        }
        let (path,header,pocket)=match (&self.placement_parent,&moved.placement_parent){
            (None,None) if self.header.stamp().same_moved_file_header(moved.header.stamp())=>
                (moved.dir.path,moved.header,None),
            (Some(old),Some(new)) if old.entity==new.entity&&old.same_mount(new)?&&self.header.stamp()==moved.header.stamp()=>
                (moved.dir.path,moved.header,moved.placement_parent),
            _=>return Err(invalid("moved FILE differs from its original outer evidence")),
        };
        // If a late physical check fails, the native result is unknown and
        // this handle is closed. This drops only the second body's EX FD.
        self.dir.path=path;self.header=header;self.placement_parent=pocket;
        self.require(new_root)
    }
    pub(crate) fn require_newborn(&self,parent:&HeldDir)->io::Result<()>{self.dir.only(Some(c"FILE"))?;self.require(parent)}
    pub(crate) fn with_placement_parent(mut self,root:&HeldDir,held:HeldDir)->io::Result<Self>{
        if self.placement_parent.is_some(){return Err(invalid("nested FILE placement not allowed"));}
        self.placement_parent=Some(held);self.require(root)?;Ok(self)
    }
    pub(crate) fn require(&self,root:&HeldDir)->io::Result<()>{
        let parent=match &self.placement_parent{Some(p)=>{
            if p.path.parent()!=Some(root.path.as_path())||!root.same_mount(p)?{return Err(invalid("different original placement root"));}
            WitnessRead::require_parent(p)?;p
        },None=>root};
        if self.dir.path.parent()!=Some(parent.path.as_path())||!parent.same_mount(&self.dir)?{return Err(invalid("different containing FILE parent"));}
        WitnessRead::require_parent(&self.dir)?;self.header.require_name(&self.dir,c"FILE")?;self.header.require_name(parent,&self.witness)
    }
}

#[path="sql_install.rs"]
mod sql_install;
#[path="move_install.rs"]
mod move_install;
pub(crate) use move_install::{MoveInstall,MoveRoom};
#[path="rename_install.rs"]
mod rename_install;
pub(crate) use rename_install::{RenameInstall,RenameStage};
#[path="parent_move.rs"]
mod parent_move;
pub(crate) use parent_move::{FlatParentMove,ParentMoveStep,lock_parents as lock_parent_pair};
#[path="packed_parent_move.rs"]
mod packed_parent_move;
pub(crate) use packed_parent_move::{PackedParentMove,PackedParentStep};
