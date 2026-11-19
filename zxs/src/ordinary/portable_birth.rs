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
    pub(crate) fn container(&self)->io::Result<CString>{super::storage_name::file_leaf(&self.name,self.file)}
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
        let result=Installed{dir,header,witness};result.require(parent)?;Ok(result)
    }
}
pub(crate) struct Installed{dir:HeldDir,header:HeldBytes,witness:CString}
impl Installed{
    pub(crate) fn directory(&self)->&HeldDir{&self.dir}
    pub(crate) fn require_newborn(&self,parent:&HeldDir)->io::Result<()>{self.dir.only(Some(c"FILE"))?;self.require(parent)}
    pub(crate) fn require(&self,parent:&HeldDir)->io::Result<()>{
        if self.dir.path.parent()!=Some(parent.path.as_path())||!parent.same_mount(&self.dir)?{return Err(invalid("different containing FILE parent"));}
        WitnessRead::require_parent(&self.dir)?;self.header.require_name(&self.dir,c"FILE")?;self.header.require_name(parent,&self.witness)
    }
}

#[path="sql_install.rs"]
mod sql_install;
