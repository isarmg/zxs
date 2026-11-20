//! The optional fixed native Web unit is one non-delegated cgroup v2
//! process, potentially with threads. There is never a cgroup-wide kill.
//! Observe original cgroup membership before signaling; require the same
//! group to drain (or its original kernel object to be removed) afterward.
use super::{bad,inactive_web_unit::UNIT};
use crate::pinned::{Entity,Mount};
use rustix::{fs::{Mode,OFlags,ResolveFlags,fstatfs,open,openat2},io::Errno,process::Pid};
use std::{ffi::CStr,fs::File,io::{self,Read,Seek},os::unix::fs::MetadataExt};

const CGROUP2_SUPER_MAGIC:u64=0x63677270; // Linux v6.12 include/uapi/linux/magic.h
const ROOT:&CStr=c"/sys/fs/cgroup";
const SLICE:&CStr=c"system.slice";
const DIR:OFlags=OFlags::PATH.union(OFlags::DIRECTORY).union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);
const SAFE:ResolveFlags=ResolveFlags::BENEATH.union(ResolveFlags::NO_XDEV).union(ResolveFlags::NO_SYMLINKS).union(ResolveFlags::NO_MAGICLINKS);
fn no()->io::Error{bad("unable to verify the original, nondelegated native Web process group")}
fn dir(f:&File)->io::Result<(Entity,u64)>{
    let m=f.metadata()?;let mount=Mount::on(f)?.ok_or_else(no)?;
    if !m.is_dir()||m.uid()!=0||m.mode()&0o022!=0||fstatfs(f)?.f_type as u64!=CGROUP2_SUPER_MAGIC{return Err(no());}
    Ok((Entity::of(&m)?,mount.id))
}
fn root()->io::Result<File>{
    let f=File::from(open(ROOT,DIR,Mode::empty())?);
    dir(&f)?;
    if f.metadata()?.ino()!=1||Mount::on(&f)?.is_none_or(|m|m.at_root!=Some(true)){return Err(no());}Ok(f)
}
fn same_member(d:&File,name:&CStr)->io::Result<File>{
    let f=File::from(openat2(d,name,OFlags::RDONLY|OFlags::NONBLOCK|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty(),SAFE)?);
    let m=f.metadata()?;let (_,mount)=dir(d)?;
    if !m.is_file()||m.uid()!=0||m.mode()&0o022!=0||fstatfs(&f)?.f_type as u64!=CGROUP2_SUPER_MAGIC||
        Mount::on(&f)?.is_none_or(|x|x.id!=mount){
        return Err(no());
    }
    Ok(f)
}
fn all(mut f:&File,max:u64)->io::Result<Vec<u8>>{
    f.rewind()?;let mut out=Vec::new();f.take(max+1).read_to_end(&mut out)?;
    if out.len() as u64>max{return Err(no());}Ok(out)
}
fn frozen_or_populated(data:&[u8])->io::Result<(bool,bool)>{
    let (population,frozen)=match data {
        b"populated 0\nfrozen 0\n"=>(false,false),b"populated 1\nfrozen 0\n"=>(true,false),
        b"populated 0\nfrozen 1\n"=>(false,true),b"populated 1\nfrozen 1\n"=>(true,true),
        _=>return Err(no()),
    };
    Ok((population,frozen))
}
pub(super) struct Original{root:File,slice:File,group:File,events:File}
impl Original{
    pub(super) fn select(pid:Pid)->io::Result<Self>{
        let root=root()?;
        let slice=File::from(openat2(&root,SLICE,DIR,Mode::empty(),SAFE)?);
        let group=File::from(openat2(&slice,UNIT,DIR,Mode::empty(),SAFE)?);
        let events=same_member(&group,c"cgroup.events")?;
        let out=Self{root,slice,group,events};out.sole_process(pid)?;Ok(out)
    }
    fn named(&self)->io::Result<bool>{
        let fresh=root()?;let a=dir(&self.root)?;
        if dir(&fresh)?!=a||dir(&self.slice)?.1!=a.1||dir(&self.group)?.1!=a.1{return Err(no());}
        let slice=File::from(openat2(&fresh,SLICE,DIR,Mode::empty(),SAFE)?);
        if dir(&slice)?!=dir(&self.slice)?{return Err(no());}
        match openat2(&slice,UNIT,DIR,Mode::empty(),SAFE){
            Ok(now)=>{if dir(&File::from(now))?==dir(&self.group)?{Ok(true)}else{Err(no())}},
            Err(Errno::NOENT)=>Ok(false),_=>Err(no()),
        }
    }
    pub(super) fn sole_process(&self,pid:Pid)->io::Result<()>{
        if !self.named()?||frozen_or_populated(&all(&self.events,128)?)?!=(true,false){return Err(no());}
        let procs=same_member(&self.group,c"cgroup.procs")?;
        let stats=same_member(&self.group,c"cgroup.stat")?;
        if all(&procs,128)?!=format!("{}\n",pid.as_raw_pid()).as_bytes()||
            !all(&stats,8192)?.starts_with(b"nr_descendants 0\n")||!self.named()?{
            return Err(no());
        }
        Ok(())
    }
    pub(super) fn drained(&self)->io::Result<bool>{
        match all(&self.events,128){
            Ok(v)=>Ok(frozen_or_populated(&v)?==(false,false)&&self.named()?),
            // For a retained kernfs events fd, ENODEV follows original
            // removal. In cgroup v2 directory rename is not supported and
            // rmdir rejects populated groups. Verify true old root/parent/
            // absent exact name as well. A newly created name is not adopted.
            Err(e) if e.raw_os_error()==Some(Errno::NODEV.raw_os_error())=>Ok(!self.named()?),
            Err(e)=>Err(e),
        }
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn bounded_kernel_values_never_treat_unknown_or_frozen_as_drained(){
        assert_eq!(frozen_or_populated(b"populated 0\nfrozen 0\n").unwrap(),(false,false));
        assert_eq!(frozen_or_populated(b"populated 1\nfrozen 0\n").unwrap(),(true,false));
        assert_eq!(frozen_or_populated(b"populated 0\nfrozen 1\n").unwrap(),(false,true));
        assert!(frozen_or_populated(b"populated 0\n").is_err());
        assert!(frozen_or_populated(b"populated 0\nfrozen 0\nextra 0\n").is_err());
    }
}
