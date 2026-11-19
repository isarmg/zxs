//! Read-only observation inside an already selected private FILE. A primary
//! name and its issued second name must still denote one actual regular file.
//! An exact copy of the whole namespace is not distinguishable by this fact.
use super::*;
use std::collections::BTreeSet;

/// Volatility, including current permissions and links. Never persist this
/// as identity. Keeping it with the very first checked OFD avoids granting a
/// new baseline across acquisition, byte admission or a slow sink callback.
#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) struct LiveStamp{
    dev:u64,ino:u64,pub(crate) len:u64,owner:u32,group:u32,mode:u32,links:u64,mtime:(i64,i64),ctime:(i64,i64),
}
impl LiveStamp{
    pub(crate) fn read(file:&File)->io::Result<Self>{
        let m=file.metadata()?;
        if !m.is_file()||m.ino()==0{return Err(invalid("portable bytes require a held regular file"));}
        Ok(Self{dev:m.dev(),ino:m.ino(),len:m.len(),owner:m.uid(),group:m.gid(),mode:m.mode(),links:m.nlink(),
            mtime:(m.mtime(),m.mtime_nsec()),ctime:(m.ctime(),m.ctime_nsec())})
    }
    pub(crate) fn same(self,other:Self)->bool{(self.dev,self.ino)==(other.dev,other.ino)}
    pub(crate) fn require(self,file:&File)->io::Result<()>{
        if Self::read(file)?!=self{return Err(invalid("portable ordinary file changed while held"));}Ok(())
    }
}
pub(crate) struct HeldBytes{file:File,original:LiveStamp}
impl HeldBytes{
    pub(crate) fn file(&self)->&File{&self.file}
    pub(crate) fn stamp(&self)->LiveStamp{self.original}
    pub(crate) fn unchanged(&self)->io::Result<()>{self.original.require(&self.file)}
    #[cfg(test)]
    pub(crate) fn separate_format_fixture(file:File)->io::Result<Self>{
        Ok(Self{original:LiveStamp::read(&file)?,file})
    }
}

/// One exact-spelling sample per direct FILE parent, not a scan per material.
/// Neither a directory listing nor a pair of names confers FILE ownership.
pub(crate) struct WitnessRead<'a>{dir:&'a HeldDir,names:BTreeSet<Vec<u8>>,mount:u64}
impl<'a> WitnessRead<'a>{
    // At peak the still-open primary O_PATH plus the checked proc directory/
    // next component needs two extra FDs beyond the returned regular pair.
    pub(crate) const OPEN_HEADROOM:usize=2;
    pub(crate) fn require_parent(dir:&HeldDir)->io::Result<()>{
        dir.require_named()?;let m=dir.file.metadata()?;
        if !m.is_dir()||m.mode()&0o7777!=0o700||m.uid()!=local_uid(){return Err(invalid("witness parent is not a private FILE directory"));}
        Ok(())
    }
    pub(crate) fn at(dir:&'a HeldDir)->io::Result<Self>{
        Self::require_parent(dir)?;let mount=mounts::id(&dir.file)?;
        let names=dir.names()?.into_iter().collect();Self::require_parent(dir)?;
        Ok(Self{dir,names,mount})
    }

    /// Only a regular data FD leaves this method. Both it and the witnesses
    /// are resolved relative to the same held parent; symlinks and devices
    /// are checked with O_PATH before any content FD can be opened.
    pub(crate) fn open(&self,main:&CStr,witness:&CStr,size:u64)->io::Result<HeldBytes>{
        if main==witness{return Err(invalid("original data and its witness must use separate names"));}
        let pin=|name:&CStr|->io::Result<File>{
            self.dir.checked_child_path(name)?;
            if !self.names.contains(name.to_bytes()){return Err(invalid("missing exact portable object spelling"));}
            self.dir.pin_record(name)?.ok_or_else(||invalid("portable material name disappeared"))
        };
        let observe=|file:&File|->io::Result<LiveStamp>{
            let stamp=LiveStamp::read(file)?;
            if stamp.owner!=local_uid()||stamp.mode&0o7777!=0o600||stamp.links!=2||
                stamp.len!=size||stamp.dev!=self.dir.entity.device||mounts::id(file)?!=self.mount{
                return Err(invalid("wrong witnessed extent or foreign mount"));}
            Ok(stamp)
        };
        let primary=pin(main)?;let expected=observe(&primary)?;
        if observe(&pin(witness)?)?!=expected{return Err(invalid("original material and its issued witness are not the same object"));}
        // SAFETY: the pinned FD was checked private, regular and on the
        // selected mount before the syscall. Never reopen an untrusted path.
        let fd=unsafe{zxs_open_pinned_record(primary.as_raw_fd(),0)};
        if fd<0{return Err(io::Error::last_os_error());}
        let held=unsafe{File::from_raw_fd(fd)};drop(primary);
        held.try_lock_shared().map_err(io::Error::from)?;
        if observe(&held)?!=expected||observe(&pin(main)?)?!=expected||observe(&pin(witness)?)?!=expected{
            return Err(invalid("original material changed while acquiring its shared lease"));}
        Ok(HeldBytes{file:held,original:expected})
    }
}
