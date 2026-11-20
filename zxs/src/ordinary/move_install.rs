//! Native preparation of one unpublished cross-volume FILE. The private
//! parent is uniquely bound by the pending transfer; its inner FILE name
//! remains the original portable convention (display plus stable FileId).
use super::{Birth,Installed};
use crate::{OperationId,fs::{HeldDir,LiveStamp,WitnessRead,invalid,local_uid},ordinary::working_round::{End,names}};
use std::{collections::BTreeSet,ffi::CStr,fs::File,io,os::{fd::{AsRawFd,FromRawFd},raw::{c_char,c_int},unix::{ffi::OsStrExt,fs::{FileExt,MetadataExt}}}};

unsafe extern "C"{
    fn zxs_create_new_regular(parent:c_int,name:*const c_char)->c_int;
    fn zxs_open_pinned_record(fd:c_int,write:c_int)->c_int;
    fn linkat(olddir:c_int,old:*const c_char,newdir:c_int,new:*const c_char,flags:c_int)->c_int;
}
fn new_file(dir:&HeldDir,leaf:&CStr)->io::Result<File>{
    dir.checked_child_path(leaf)?;
    // SAFETY: Both actual sealed parent and the checked C string are live.
    let fd=unsafe{zxs_create_new_regular(dir.file.as_raw_fd(),leaf.as_ptr())};
    if fd<0{return Err(io::Error::last_os_error());}
    let file=unsafe{File::from_raw_fd(fd)};let m=file.metadata()?;
    if !m.is_file()||m.ino()==0||m.uid()!=local_uid()||m.mode()&0o7777!=0o600||
        m.nlink()!=1||m.len()!=0||!dir.shares_file_mount(&file)?{
        return Err(invalid("not a new original ordinary transfer file"));
    }
    Ok(file)
}
fn link(dir:&HeldDir,from:&CStr,target:&HeldDir,to:&CStr)->io::Result<()>{
    dir.checked_child_path(from)?;target.checked_child_path(to)?;
    // No replacement. Both names are relative to their exact held parents.
    if unsafe{linkat(dir.file.as_raw_fd(),from.as_ptr(),target.file.as_raw_fd(),to.as_ptr(),0)}<0{
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn sample(dir:&HeldDir)->io::Result<BTreeSet<Vec<u8>>>{Ok(dir.names()?.into_iter().collect())}

#[derive(Clone,Copy,Eq,PartialEq)]
pub(crate) enum MoveRoom{Present,Vacant,Full}
#[derive(Debug)]
enum LostStage{Outer,Pair(std::ffi::CString,std::ffi::CString)}
impl std::fmt::Display for LostStage{
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("incomplete original transfer placement; material left in place")}
}
impl std::error::Error for LostStage{}
impl From<LostStage> for io::Error{
    fn from(v:LostStage)->Self{Self::new(io::ErrorKind::InvalidData,v)}
}
pub(crate) struct MoveInstall{body:Installed}
impl MoveInstall{
    /// Keep physical absence separate from a true exhausted namespace.
    /// Neither a full known root nor an I/O error is a positive stage.
    pub(crate) fn room(root:&HeldDir,op:OperationId)->io::Result<MoveRoom>{
        let pocket=Birth::placement_parent(op)?;let observed=root.names()?;
        if root.exact_presence(&observed,&pocket)?{return Ok(MoveRoom::Present);}
        let bytes:usize=observed.iter().map(|x|x.len()+1).sum();
        if observed.len()>=crate::fs::DIRECTORY_NAMES||bytes+1+pocket.to_bytes().len()>crate::fs::DIRECTORY_BYTES{
            return Ok(MoveRoom::Full);
        }
        Ok(MoveRoom::Vacant)
    }
    /// The placement parent is made under the short namespace lock, before
    /// any bulk I/O. An incomplete first two-name link is never adopted.
    /// The caller owns the whole original source FILE EX, a live pending
    /// ticket and both independently qualified volume roots.
    pub(crate) fn attach(root:&HeldDir,birth:&Birth,op:OperationId,display:&str)->io::Result<Self>{
        let pocket=Birth::placement_parent(op)?;
        let leaf=Birth::placed_container(display,birth.file)?;let witness=birth.witness()?;
        let observed=root.names()?;
        let stage=if let Some(stage)=root.directory_in_sample(&pocket,&observed.iter().cloned().collect())?{
            stage
        }else{
            if root.exact_presence(&observed,&pocket)?{return Err(invalid("transfer placement name not vacant"));}
            let bytes:usize=observed.iter().map(|x|x.len()+1).sum();
            if observed.len()>=crate::fs::DIRECTORY_NAMES||bytes+1+pocket.to_bytes().len()>crate::fs::DIRECTORY_BYTES{
                return Err(io::Error::new(io::ErrorKind::StorageFull,"no room for a new transfer placement"));
            }
            let stage=root.new_child_directory(&pocket)?;stage.only(None)?;
            let body=stage.new_child_directory(&leaf)?;body.only(None)?;
            let header=new_file(&body,c"FILE")?;
            header.write_all_at(&birth.header(),0)?;header.sync_all()?;
            link(&body,c"FILE",&stage,&witness)?;
            header.sync_all()?;body.file.sync_all()?;stage.file.sync_all()?;root.file.sync_all()?;
            stage
        };
        let ls=sample(&stage)?;
        if ls!=BTreeSet::from([leaf.to_bytes().to_vec(),witness.to_bytes().to_vec()]){
            // The same exact chosen private directory is present, but its
            // proof was never completed, or is gone. No file here is adopted.
            stage.require_named()?;root.require_named()?;
            return Err(LostStage::Outer.into());
        }
        let body=birth.working_records_in(&stage,&ls,&leaf,true,|_|Ok(()))?
            .with_placement_parent(root,stage)?;
        Ok(Self{body})
    }
    pub(crate) fn body(&self)->&HeldDir{self.body.directory()}
    pub(crate) fn retirement_target(&self,root:&HeldDir,used:&crate::ordinary::working_round::Used)->io::Result<crate::fs::RetireProof>{self.body.verified_retirement_target(root,used)}
    pub(crate) fn require(&self,root:&HeldDir)->io::Result<()>{self.body.require(root)}
    /// Create only missing pairs, still zero bytes. Verify no other member
    /// before any material is copied. A retry may compare already written
    /// bytes; it must not adopt an unpaired role or truncate any material.
    pub(crate) fn prepare_pairs(&self,rounds:&[(OperationId,End)])->io::Result<BTreeSet<Vec<u8>>>{
        let dir=self.body();let known=sample(dir)?;let mut allowed=BTreeSet::from([b"FILE".to_vec()]);
        for (op,_) in rounds{for w in names(*op){if !allowed.insert(w.into_bytes()){
            return Err(invalid("two transfer rounds claim the same name"));
        }}}
        if !known.is_subset(&allowed)||known.len()>crate::fs::DIRECTORY_NAMES||
           allowed.len()>crate::fs::DIRECTORY_NAMES||allowed.iter().map(|x|1+x.len()).sum::<usize>()>crate::fs::DIRECTORY_BYTES{
            return Err(invalid("target transfer has unknown members or exceeds the native budget"));
        }
        let scope=WitnessRead::at(dir)?;
        for (op,end) in rounds{
            let [raw,raw_witness,checks,checks_witness]=names(*op);
            for (a,b,n) in [(&raw,&raw_witness,end.data),(&checks,&checks_witness,end.checks)]{
                let left=known.contains(a.to_bytes());let right=known.contains(b.to_bytes());
                if left!=right{
                    let current=dir.names()?;
                    if dir.exact_presence(&current,a)?!=dir.exact_presence(&current,b)?{
                        return Err(LostStage::Pair(a.clone(),b.clone()).into());
                    }
                    return Err(invalid("transfer role names changed during selection"));
                }
                if left{
                    let file=dir.pin_record(a)?.ok_or_else(||invalid("transfer role disappeared"))?;
                    let len=LiveStamp::read(&file)?.len;
                    if len>n{return Err(invalid("unpublished transfer extent exceeds the committed cut"));}
                    scope.open(a,b,len)?;
                }else{
                    let fresh=new_file(dir,a)?;
                    link(dir,a,dir,b)?;fresh.sync_all()?;
                }
            }
        }
        dir.file.sync_all()?;
        WitnessRead::at(dir)?.claimed_work(&allowed,&allowed)?;Ok(allowed)
    }
    pub(crate) fn outer_incomplete(error:&io::Error)->bool{
        matches!(error.get_ref().and_then(|e|e.downcast_ref::<LostStage>()),Some(LostStage::Outer))
    }
    pub(crate) fn pair_incomplete(&self,error:&io::Error,root:&HeldDir)->io::Result<bool>{
        let Some(LostStage::Pair(a,b))=error.get_ref().and_then(|e|e.downcast_ref::<LostStage>()) else{return Ok(false);};
        self.require(root)?;let dir=self.body();let names=dir.names()?;
        let incomplete=dir.exact_presence(&names,a)?!=dir.exact_presence(&names,b)?;
        self.require(root)?;Ok(incomplete)
    }
    pub(crate) fn open_pair(&self,scope:&WitnessRead<'_>,round:OperationId,end:End)->io::Result<(File,File)>{
        let dir=self.body();let names=names(round);
        let open=|a:&CStr,b:&CStr,n:u64|->io::Result<File>{
            let pin=dir.pin_record(a)?.ok_or_else(||invalid("missing original unpublished transfer role"))?;
            let len=LiveStamp::read(&pin)?.len;
            if len>n{return Err(invalid("unpublished transfer role grew past committed lead"));}
            let hold=scope.open(a,b,len)?;
            // SAFETY: an actual witnessed ordinary FD, no native path.
            let fd=unsafe{zxs_open_pinned_record(hold.file().as_raw_fd(),1)};
            if fd<0{return Err(io::Error::last_os_error());}
            let course=unsafe{File::from_raw_fd(fd)};
            hold.stamp().require(&course)?;hold.unchanged()?;
            Ok(course)
        };
        let raw=open(&names[0],&names[1],end.data)?;
        let checks=open(&names[2],&names[3],end.checks)?;
        if LiveStamp::read(&raw)?.same(LiveStamp::read(&checks)?){return Err(invalid("transfer roles are the same object"));}
        Ok((raw,checks))
    }
    pub(crate) fn sync_pair(&self,data:&File,checks:&File,end:End)->io::Result<()>{
        for (fd,n) in [(data,end.data),(checks,end.checks)]{
            let a=LiveStamp::read(fd)?;
            if a.len>n{return Err(invalid("transfer appears longer than the original confirmed window"));}
            if a.len<n{a.require(fd)?;fd.set_len(n)?;} // remaining unselected hole only
            fd.sync_all()?;
        }
        self.body().file.sync_all()
    }
    pub(crate) fn require_members(&self,root:&HeldDir,allowed:&BTreeSet<Vec<u8>>)->io::Result<()>{
        self.require(root)?;
        let parent=self.body.placement_parent.as_ref().ok_or_else(||invalid("no original transfer placement parent"))?;
        let leaf=self.body().path.file_name().ok_or_else(||invalid("no portable transfer FILE name"))?;
        if sample(parent)?!=BTreeSet::from([leaf.as_bytes().to_vec(),self.body.witness.to_bytes().to_vec()]){
            return Err(invalid("unexpected outer member in original transfer placement"));
        }
        WitnessRead::at(self.body())?.claimed_work(allowed,allowed)?;self.require(root)
    }
    pub(crate) fn finish(&self,root:&HeldDir,allowed:&BTreeSet<Vec<u8>>)->io::Result<()>{
        self.require_members(root,allowed)?;
        self.body.header.file().sync_all()?;self.body().file.sync_all()?;
        self.body.placement_parent.as_ref().ok_or_else(||invalid("no original transfer placement parent"))?.file.sync_all()?;
        root.file.sync_all()?;self.require(root)
    }
}
