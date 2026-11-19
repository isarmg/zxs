//! Materialize one *already reserved* SQLite root birth. Before both original
//! names exist a crash can leave an ambiguous object. It is never adopted,
//! deleted, overwritten or taken as authority. The SQL reservation bounds it.
use super::{Birth,Installed};
use crate::fs::{HeldDir,WitnessRead,invalid};
use std::{collections::BTreeSet,ffi::CStr,fs::File,io,os::{fd::{AsRawFd,FromRawFd},raw::{c_char,c_int},unix::fs::{FileExt,MetadataExt}}};

unsafe extern "C"{
    fn zxs_create_new_regular(parent:c_int,name:*const c_char)->c_int;
    fn linkat(olddir:c_int,old:*const c_char,newdir:c_int,new:*const c_char,flags:c_int)->c_int;
    fn zxs_rename_fresh(olddir:c_int,old:*const c_char,newdir:c_int,new:*const c_char)->c_int;
}
fn os(n:c_int)->io::Result<()>{if n<0{Err(io::Error::last_os_error())}else{Ok(())}}
fn sample(f:&HeldDir)->io::Result<BTreeSet<Vec<u8>>>{
    let raw=f.names()?;let len=raw.len();let names:BTreeSet<_>=raw.into_iter().collect();
    if names.len()!=len{return Err(invalid("ambiguous native parent enumeration"));}Ok(names)
}

/// Check both native occupancy (including aliases) and the existing real
/// enumeration budget before reserving exactly these names. The final name
/// replaces the stage name; its peak cost is the larger, not their sum.
impl Birth{
    pub(crate) fn new_root_vacancy(&self,parent:&HeldDir,names:&[Vec<u8>])->io::Result<()>{
        use crate::fs::{DIRECTORY_NAMES,DIRECTORY_BYTES};
        let stage=self.lower(self.slot)?;let pin=self.witness()?;let end=self.container()?;
        for leaf in [&stage,&pin,&end]{
            if parent.exact_presence(names,leaf)?{return Err(invalid("occupied ordinary FILE or reserved physical name"));}
        }
        let used:usize=names.iter().map(|x|x.len()+1).sum();
        if names.len()+2>DIRECTORY_NAMES||used+2+pin.to_bytes().len()+end.to_bytes().len().max(stage.to_bytes().len())>DIRECTORY_BYTES{
            return Err(invalid("new FILE membership exceeds the parent's actual enumeration budget"));}
        Ok(())
    }

}
impl crate::system::reader::smb::file_create::Reservation<'_>{
    /// Called only after SQL reserved this operation, real parent, stable IDs,
    /// exact stage/terminal/witness. The parent is held exclusively throughout;
    /// never replace a native name, never open R. No SQL lock during sync.
    pub(crate) fn make_ready(&self)->io::Result<Installed>{
        let (birth,parent)=self.original();
        let stage=birth.lower(birth.slot)?;let end=birth.container()?;let pin=birth.witness()?;
        let before=sample(parent)?;
        let stage_exists=parent.directory_in_sample(&stage,&before)?.is_some();
        let final_exists=parent.directory_in_sample(&end,&before)?.is_some();
        if final_exists{
            if stage_exists{return Err(invalid("both reserved FILE names exist"));}
            let held=birth.installed(parent,&before)?;
            held.header.file().sync_all()?;held.dir.file.sync_all()?;parent.file.sync_all()?;held.require(parent)?;return Ok(held);
        }
        let mut original=if !stage_exists{
            birth.new_root_vacancy(parent,&before.into_iter().collect::<Vec<_>>())?;
            let d=parent.new_child_directory(&stage)?;WitnessRead::require_parent(&d)?;d.only(None)?;
            // Sole writer is under the exact ordinary root namespace lock.
            let n=unsafe{zxs_create_new_regular(d.file.as_raw_fd(),c"FILE".as_ptr())};
            if n<0{return Err(io::Error::last_os_error());}let raw=unsafe{File::from_raw_fd(n)};
            let m=raw.metadata()?;
            if !m.is_file()||m.mode()&0o7777!=0o600||m.uid()!=crate::fs::local_uid()||m.nlink()!=1||
                m.len()!=0||!d.shares_file_mount(&raw)?{return Err(invalid("not the fresh local FILE header"));}
            raw.write_all_at(&birth.header(),0)?;raw.sync_all()?;WitnessRead::require_parent(&d)?;
            os(unsafe{linkat(d.file.as_raw_fd(),c"FILE".as_ptr(),parent.file.as_raw_fd(),pin.as_ptr(),0)})?;
            raw.sync_all()?;d.file.sync_all()?;parent.file.sync_all()?;
            let check=birth.newborn_at(parent,&sample(parent)?,&stage)?;
            let observed=check.header.file().metadata()?;
            if (observed.dev(),observed.ino())!=(m.dev(),m.ino()){return Err(invalid("new FILE header was replaced before binding"));}
            check
        }else{birth.newborn_at(parent,&before,&stage)?};
        // Cold recovery reaches here only with BOTH exact links to a private
        // FILE and the header bytes of this actual SQL reservation.
        original.header.file().sync_all()?;original.dir.file.sync_all()?;parent.file.sync_all()?;
        original.require(parent)?;
        if parent.entry_present(&end)?{return Err(invalid("FILE destination no longer vacant"));}
        let destination=parent.checked_child_path(&end)?;
        os(unsafe{zxs_rename_fresh(parent.file.as_raw_fd(),stage.as_ptr(),parent.file.as_raw_fd(),end.as_ptr())})?;
        original.dir.path=destination;parent.file.sync_all()?;
        if parent.entry_present(&stage)?{return Err(invalid("old stage name reappeared"));}
        original.require(parent)?;Ok(original)
    }
}
