//! Read-only observation inside an already selected private FILE. A primary
//! name and its issued second name must still denote one actual regular file.
//! An exact copy of the whole namespace is not distinguishable by this fact.
use super::*;
use std::{collections::BTreeSet,ops::RangeInclusive};

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
    pub(super) fn retirement_stamp(self)->[u8;56]{
        // Only originally checked named pairs pass this into a retirement
        // guard. Mount identity is separately verified against the volume.
        let mut b=[0;56];
        for (i,v) in [self.ino,self.len,self.mtime.0 as u64,self.mtime.1 as u64,self.ctime.0 as u64,self.ctime.1 as u64].into_iter().enumerate(){
            b[i*8..i*8+8].copy_from_slice(&v.to_be_bytes());
        }
        b[48..52].copy_from_slice(&self.owner.to_be_bytes());b[52..56].copy_from_slice(&self.group.to_be_bytes());b
    }
    pub(crate) fn same(self,other:Self)->bool{(self.dev,self.ino)==(other.dev,other.ino)}
    /// A controlled rename of the FILE header's outer hard link may
    /// change its inode ctime. No data or other stamp field is relaxed.
    pub(crate) fn same_moved_file_header(self,now:Self)->bool{
        Self{ctime:now.ctime,..self}==now
    }
    pub(crate) fn require(self,file:&File)->io::Result<()>{
        if Self::read(file)?!=self{return Err(invalid("portable ordinary file changed while held"));}Ok(())
    }
}
#[derive(Clone,Copy,Eq,PartialEq)]
enum Extent{Whole,CheckedPrefix}
pub(crate) struct HeldBytes{file:File,original:LiveStamp,extent:Extent}
impl HeldBytes{
    pub(crate) fn duplicate(&self)->io::Result<Self>{
        Ok(Self{file:self.file.try_clone()?,original:self.original,extent:self.extent})
    }
    pub(crate) fn file(&self)->&File{&self.file}
    pub(crate) fn stamp(&self)->LiveStamp{self.original}
    /// Use only for a working extent with independently authenticated bytes.
    /// FILE headers, formal material and candidate readback stay Whole.
    /// The caller must keep its own confirmed endpoints; the opening length
    /// may already include part of an unconfirmed tail.
    pub(crate) fn checked_prefix(self)->io::Result<Self>{
        self.unchanged()?;Ok(Self{extent:Extent::CheckedPrefix,..self})
    }
    fn require_object(&self,file:&File)->io::Result<()>{
        let old=self.original;
        if self.extent==Extent::Whole{return old.require(file);}
        let now=LiveStamp::read(file)?;
        if now==old{return Ok(());}
        if !now.same(old)||now.len<=old.len||now.owner!=old.owner||now.group!=old.group||
            now.mode!=old.mode||now.links!=old.links{return Err(invalid("selected working prefix lost its original object or extent"));}
        Ok(())
    }
    pub(crate) fn unchanged(&self)->io::Result<()>{self.require_object(&self.file)}
    pub(crate) fn require_name(&self,parent:&HeldDir,name:&CStr)->io::Result<()>{
        let p=parent.pin_record(name)?.ok_or_else(||invalid("portable original lost a required name"))?;
        if !parent.shares_file_mount(&p)?{return Err(invalid("portable original name is on a different mount"));}
        self.require_object(&p)?;self.unchanged()
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

    pub(crate) fn matching_presence(&self,primary:&CStr,second:&CStr)->io::Result<()>{
        if self.names.contains(primary.to_bytes())!=self.names.contains(second.to_bytes()){
            return Err(invalid("unproven single working role name; do not create or adopt its companion"));}
        Ok(())
    }
    pub(crate) fn present_work_roles(&self,roles:&[CString;4])->u8{
        roles.iter().enumerate().fold(0,|mask,(i,n)|if self.names.contains(n.to_bytes()){mask|(1u8<<i)}else{mask})
    }
    pub(crate) fn work_members(&self,roles:&[CString;4],complete:bool,retained:&BTreeSet<Vec<u8>>)->io::Result<()>{
        if !self.names.contains(c"FILE".to_bytes())||self.names.iter().any(|n|n.as_slice()!=c"FILE".to_bytes()&&!retained.contains(n)&&!roles.iter().any(|r|r.to_bytes()==n.as_slice()))||
            (complete&&self.present_work_roles(roles)!=15){return Err(invalid("unexpected or incomplete member set of the working FILE"));}Ok(())
    }
    pub(crate) fn claimed_work(&self,needed:&BTreeSet<Vec<u8>>,permitted:&BTreeSet<Vec<u8>>)->io::Result<()>{
        if !self.names.contains(c"FILE".to_bytes())||!needed.is_subset(&self.names)||
            self.names.iter().any(|n|n.as_slice()!=c"FILE".to_bytes()&&!permitted.contains(n)){
            return Err(invalid("original FILE contains unissued or missing work member names"));
        }
        Ok(())
    }
    pub(crate) fn only_header(&self)->io::Result<()>{
        if self.names.len()!=1||!self.names.contains(c"FILE".to_bytes()){return Err(invalid("unissued or missing member in newborn FILE"));}
        Ok(())
    }
    /// Only a regular data FD leaves this method. Both it and the witnesses
    /// are resolved relative to the same held parent; symlinks and devices
    /// are checked with O_PATH before any content FD can be opened.
    pub(crate) fn open(&self,main:&CStr,witness:&CStr,size:u64)->io::Result<HeldBytes>{
        if main==witness{return Err(invalid("original data and its witness must use separate names"));}
        self.open_pair(main,self.dir,&self.names,witness,size..=size)
    }
    /// Only the revision selector may supply the independently issued lower
    /// and upper extents. The actual file length does not select a revision.
    pub(crate) fn open_work_prefix(&self,main:&CStr,witness:&CStr,confirmed:u64,reserved:u64)->io::Result<HeldBytes>{
        if main==witness||confirmed>reserved||reserved>i64::MAX as u64{return Err(invalid("unusable original work prefix range"));}
        self.open_pair(main,self.dir,&self.names,witness,confirmed..=reserved)?.checked_prefix()
    }
    /// An installed FILE uses its existing inner header as the primary.
    /// Its single issued second name sits in the exact immediate parent;
    /// no directory hardlink, third metadata database or persistent inode ID.
    pub(crate) fn file_header(&self,parent:&HeldDir,sample:&BTreeSet<Vec<u8>>,witness:&CStr,size:u64)->io::Result<HeldBytes>{
        self.directory_header(parent,sample,c"FILE",witness,size)
    }
    /// Only for an already-issued same-mount FILE move between two
    /// actual held parents. The original outer link may have moved before
    /// its body; ordinary opens still require the same immediate parent.
    pub(crate) fn moved_file_header(&self,body_parent:&HeldDir,link_parent:&HeldDir,
        link_names:&BTreeSet<Vec<u8>>,witness:&CStr,size:u64)->io::Result<HeldBytes>{
        if self.dir.path.parent()!=Some(body_parent.path.as_path())||
            !self.dir.same_mount(body_parent)?||!body_parent.same_mount(link_parent)?{
            return Err(invalid("FILE move header crosses its original held mount or parent"));
        }
        body_parent.require_named()?;link_parent.require_named()?;
        let held=self.open_pair(c"FILE",link_parent,link_names,witness,size..=size)?;
        body_parent.require_named()?;link_parent.require_named()?;
        Self::require_parent(self.dir)?;held.unchanged()?;Ok(held)
    }
    pub(super) fn directory_header(&self,parent:&HeldDir,sample:&BTreeSet<Vec<u8>>,primary:&CStr,witness:&CStr,size:u64)->io::Result<HeldBytes>{
        if self.dir.path.parent()!=Some(parent.path.as_path())||!self.dir.same_mount(parent)?{
            return Err(invalid("directory header witness is not in its actual containing parent"));}
        parent.require_named()?;let held=self.open_pair(primary,parent,sample,witness,size..=size)?;
        parent.require_named()?;Self::require_parent(self.dir)?;held.unchanged()?;Ok(held)
    }
    fn open_pair(&self,main:&CStr,other:&HeldDir,names:&BTreeSet<Vec<u8>>,witness:&CStr,sizes:RangeInclusive<u64>)->io::Result<HeldBytes>{
        let pin=|dir:&HeldDir,names:&BTreeSet<Vec<u8>>,name:&CStr|->io::Result<File>{
            dir.checked_child_path(name)?;
            if !names.contains(name.to_bytes()){return Err(invalid("missing exact portable object spelling"));}
            dir.pin_record(name)?.ok_or_else(||invalid("portable material name disappeared"))
        };
        let observe=|file:&File|->io::Result<LiveStamp>{
            let stamp=LiveStamp::read(file)?;
            if stamp.owner!=local_uid()||stamp.mode&0o7777!=0o600||stamp.links!=2||
                !sizes.contains(&stamp.len)||stamp.dev!=self.dir.entity.device||mounts::id(file)?!=self.mount{
                return Err(invalid("wrong witnessed extent or foreign mount"));}
            Ok(stamp)
        };
        let primary=pin(self.dir,&self.names,main)?;let expected=observe(&primary)?;
        if observe(&pin(other,names,witness)?)?!=expected{return Err(invalid("original material and its issued witness are not the same object"));}
        // SAFETY: the pinned FD was checked private, regular and on the
        // selected mount before the syscall. Never reopen an untrusted path.
        let fd=unsafe{zxs_open_pinned_record(primary.as_raw_fd(),0)};
        if fd<0{return Err(io::Error::last_os_error());}
        let held=unsafe{File::from_raw_fd(fd)};drop(primary);
        held.try_lock_shared().map_err(io::Error::from)?;
        if observe(&held)?!=expected||observe(&pin(self.dir,&self.names,main)?)?!=expected||observe(&pin(other,names,witness)?)?!=expected{
            return Err(invalid("original material changed while acquiring its shared lease"));}
        Ok(HeldBytes{file:held,original:expected,extent:Extent::Whole})
    }
}

#[cfg(test)]
include!("../../tests/unit/attached/fs/witnessed/items.rs");
