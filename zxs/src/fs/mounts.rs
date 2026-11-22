//! A bind mount of a subdirectory can hide a marked parent from openat("..").
//! Inspect the kernel's mount-root field for every mount in the held lineage.
//! Unknown mount IDs or an exposed subtree are not a proof of complete ancestry.
use super::{invalid, File};
use std::{collections::BTreeMap, io::{self,Read}, os::fd::{AsRawFd,FromRawFd}};

unsafe extern "C" {
    fn zxs_mount_id(fd:std::os::raw::c_int, out:*mut u64)->std::os::raw::c_int;
    fn zxs_open_mountinfo()->std::os::raw::c_int;
}

pub(super) fn id(file:&File)->io::Result<u64>{
    let mut value=0u64;
    // SAFETY: held FD and initialized writable result for statx.
    if unsafe{zxs_mount_id(file.as_raw_fd(),&mut value)}<0{return Err(io::Error::last_os_error());}
    if value==0{return Err(invalid("kernel returned no mount identity"));}
    Ok(value)
}

#[derive(Clone,Copy,Eq,PartialEq)]
struct MountInfo{whole:bool,superblock:(u32,u32)}
pub(super) struct Mounts(BTreeMap<u64,MountInfo>);
impl Mounts {
    pub(super) fn read()->io::Result<Self>{
        // SAFETY: native shim returns a private FD on the process' procfs.
        let fd=unsafe{zxs_open_mountinfo()};
        if fd<0{return Err(io::Error::last_os_error());}
        let mut bytes=Vec::new();
        unsafe{File::from_raw_fd(fd)}.take(4*1024*1024+1).read_to_end(&mut bytes)?;
        Self::parse(&bytes)
    }
    fn parse(bytes:&[u8])->io::Result<Self>{
        let bad=||invalid("cannot establish kernel mount ancestry");
        if bytes.is_empty()||bytes.len()>4*1024*1024||bytes.last()!=Some(&b'\n'){return Err(bad());}
        let mut table=BTreeMap::new();
        for line in bytes[..bytes.len()-1].split(|&c|c==b'\n'){
            let mut fields=line.split(|&c|c==b' ');
            let number=fields.next().and_then(|v|std::str::from_utf8(v).ok()).and_then(|s|s.parse::<u64>().ok()).filter(|n|*n!=0).ok_or_else(bad)?;
            // Field 3 is the mounted superblock's device; field 4 is the
            // exact root as exported by the kernel, not the mountpoint.
            let device=fields.nth(1).and_then(|v|std::str::from_utf8(v).ok()).and_then(|s|s.split_once(':')).ok_or_else(bad)?;
            let superblock=(device.0.parse::<u32>().map_err(|_|bad())?,device.1.parse::<u32>().map_err(|_|bad())?);
            let root=fields.next().ok_or_else(bad)?;
            let info=MountInfo{whole:root==b"/",superblock};
            if !root.starts_with(b"/") || table.len()>=16384 || table.insert(number,info).is_some(){return Err(bad());}
        }
        Ok(Self(table))
    }
    pub(super) fn complete(&self,file:&File)->io::Result<()>{
        if self.0.get(&id(file)?).is_none_or(|p|!p.whole){return Err(invalid("mount hides a physical ancestor or changed during selection"));}
        Ok(())
    }
    /// Compare superblocks: bind mounts can have different mount IDs and
    /// still refer to the same filesystem.
    pub(super) fn distinct_filesystems(&self,a:&File,b:&File)->io::Result<bool>{self.separate(id(a)?,id(b)?)}
    fn separate(&self,a:u64,b:u64)->io::Result<bool>{
        let get=|i|self.0.get(&i).map(|p|p.superblock).ok_or_else(||invalid("held filesystem is not in this mount namespace"));
        Ok(get(a)?!=get(b)?)
    }
}

#[cfg(test)]
#[path="../../tests/unit/mounts.rs"]
mod tests;
