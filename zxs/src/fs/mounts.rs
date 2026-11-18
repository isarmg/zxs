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

pub(super) struct Mounts(BTreeMap<u64,bool>);
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
            // mountinfo fields 2,3 are parent and device; 4 is the exact root
            // as exported by the kernel, escaped as required, not mountpoint.
            let root=fields.nth(2).ok_or_else(bad)?;
            if !root.starts_with(b"/") || table.len()>=16384 || table.insert(number,root==b"/").is_some(){return Err(bad());}
        }
        Ok(Self(table))
    }
    pub(super) fn complete(&self,file:&File)->io::Result<()>{
        if self.0.get(&id(file)?)!=Some(&true){return Err(invalid("mount hides a physical ancestor or changed during selection"));}
        Ok(())
    }
}

#[cfg(test)]
#[path="../../tests/unit/mounts.rs"]
mod tests;
