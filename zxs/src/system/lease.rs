//! Real online lifetime exclusion. The default business actor holds this one
//! independently opened S-directory OFD. Never use a raw DB FD for locking,
//! sampling or reconnect; only the fixed SQLite VFS is allowed to open it.
use super::{DB,JOURNAL,ROOT_MARK};
use crate::fs::{ComponentLimit,ControlRead,HeldDir,NameBudget,invalid};
use std::{ffi::{CStr,CString},io,os::{fd::AsRawFd,raw::{c_char,c_int},unix::ffi::OsStrExt},path::Path};

unsafe extern "C"{
    fn zxs_sql_private_name(parent:c_int,name:*const c_char,ino:*mut u64)->c_int;
}
fn private_name(root:&HeldDir,n:&CStr)->io::Result<u64>{
    let mut ino=0;
    // SAFETY: statx takes an owned open directory, a fixed terminated leaf
    // and our writable integer. It never opens or closes the named object.
    if unsafe{zxs_sql_private_name(root.file.as_raw_fd(),n.as_ptr(),&mut ino)}<0{
        return Err(io::Error::last_os_error());
    }
    Ok(ino)
}
pub(super) struct SystemDir{pub(super) root:HeldDir,db:CString,ino:u64,marker_ino:u64}
impl SystemDir{
    pub(super) fn hold(path:&Path,vfs_path_limit:usize)->io::Result<Self>{
        let root=HeldDir::trusted_system(path)?;
        root.share()?; // Fresh OFD specific to this online holder; no EX, conversion or unlock.
        let b=NameBudget::held(&root)?;b.child(DB)?;b.child(JOURNAL)?;b.child(ROOT_MARK)?;
        let jr=root.checked_child_path(JOURNAL)?;
        if !jr.as_os_str().as_bytes().len().checked_add(1).is_some_and(|n|n<=vfs_path_limit){
            return Err(invalid("SYSTEM DB/journal path exceeds this exact SQLite VFS limit"));
        }
        let db=CString::new(root.checked_child_path(DB)?.as_os_str().as_bytes()).map_err(|_|invalid("bad DB path"))?;
        let ino=private_name(&root,DB)?;let marker_ino=private_name(&root,ROOT_MARK)?;
        if ino==marker_ino{return Err(invalid("DB and root marker are not distinct regular files"));}
        let result=Self{root,db,ino,marker_ino};result.require()?;Ok(result)
    }
    /// Stat first: a root marker is an ordinary control file, never an
    /// alias through which to open/close the active SQLite DB inode.
    pub(super) fn hold_marker(&self, selected:&HeldDir)->io::Result<ControlRead>{
        let i=private_name(selected,ROOT_MARK)?;
        if private_name(&self.root,DB)?!=self.ino ||
            (selected.entity.device==self.root.entity.device && i==self.ino){
            return Err(invalid("a volume marker cannot open a database alias"));
        }
        ControlRead::open(selected,ROOT_MARK,101)
    }
    /// Two live independently locked directory OFDs, with the same original
    /// DB and marker names. statx only: never open the DB to compare inodes.
    pub(super) fn matches_held(&self,other:&Self)->io::Result<()>{
        self.require()?;other.require()?;
        if self.root.entity!=other.root.entity||!self.root.same_mount(&other.root)?||
            self.ino!=other.ino||self.marker_ino!=other.marker_ino{return Err(invalid("different original SYSTEM"));}
        Ok(())
    }
    pub(super) fn path(&self)->&CStr{&self.db}
    pub(super) fn component_limit(&self)->io::Result<ComponentLimit>{ComponentLimit::held(&self.root)}
    pub(super) fn require(&self)->io::Result<()>{
        let now=HeldDir::trusted_system(&self.root.path)?;
        if now.entity!=self.root.entity || !now.same_mount(&self.root)? ||
           private_name(&self.root,DB)?!=self.ino || private_name(&self.root,ROOT_MARK)?!=self.marker_ino {
            return Err(invalid("SYSTEM directory, database name or marker changed"));
        }
        let names=self.root.names()?;let mut db=false;let mut mark=false;
        for n in &names{
            if n==DB.to_bytes(){db=true;}else if n==ROOT_MARK.to_bytes(){mark=true;}
            else if n==JOURNAL.to_bytes(){
                // A legitimate writer can finish between readdir and statx.
                // SQLite itself decides hotness and owns all recovery.
                match private_name(&self.root,JOURNAL){Ok(_)=>{},Err(e)if e.kind()==io::ErrorKind::NotFound=>{},Err(e)=>return Err(e)}
            }else{return Err(invalid("unknown SYSTEM member, foreign family or WAL; do not repair"));}
        }
        if !db||!mark{return Err(invalid("missing exact SYSTEM DB/marker spelling"));}Ok(())
    }
}
