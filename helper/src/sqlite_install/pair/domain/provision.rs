//! Shared offline origin attributes, names and small sealed messages.
mod return_to_host;
mod live;
pub(in crate::sqlite_install::pair) use return_to_host::{give_live_approval,request_live_approval,RustSite,VaultStamp,
    checked_vault_stamp,verify_fresh_worker,Allocated,finish_seen_pending,verify_fresh_vault,FreshHome,FreshParent};
pub(in crate::sqlite_install) use return_to_host::{lease as require_lent_ex,shared_lease as require_lent_sh};
use super::bad;
use crate::pinned::{Entity,Mount};
use rustix::fs::{Mode,OFlags,RawDir,openat};
use std::{fs::File,io::{self,Seek,Write},mem::MaybeUninit,os::unix::fs::MetadataExt};

fn mark(fd:&File)->io::Result<(Entity,u64)>{
    Ok((Entity::of(&fd.metadata()?)?,Mount::on(fd)?.ok_or_else(||bad("missing original mount identity"))?.id))
}
fn child(parent:&File,name:&std::ffi::CStr)->io::Result<File>{
    Ok(File::from(openat(parent,name,OFlags::RDONLY|OFlags::DIRECTORY|OFlags::NONBLOCK|OFlags::CLOEXEC|OFlags::NOFOLLOW,Mode::empty())?))
}
#[derive(Clone,Copy,Eq,PartialEq)]
struct Times{links:u64,len:u64,mtime:(i64,i64),ctime:(i64,i64)}
impl Times{fn on(f:&File)->io::Result<Self>{let m=f.metadata()?;Ok(Self{links:m.nlink(),len:m.len(),
    mtime:(m.mtime(),m.mtime_nsec()),ctime:(m.ctime(),m.ctime_nsec())})}}
fn contents(dir:&File,expected:&[&[u8]])->io::Result<()>{
    // All lists are small fixed constants. A raw, complete inventory rejects
    // case-folded aliases and previously allocated partial or final outputs.
    if expected.len()>8 {return Err(bad("invalid dedicated directory inventory"));}
    let before=Times::on(dir)?;let other=child(dir,c".")?;
    if before.links==0||mark(&other)?!=mark(dir)?{return Err(bad("cannot attribute the original directory"));}
    let mut buf=[MaybeUninit::uninit();4096];let mut it=RawDir::new(&other,&mut buf);let mut seen=0u16;
    while let Some(row)=it.next(){let row=row?;let bytes=row.file_name().to_bytes();
        if matches!(bytes,b"."|b".."){continue;}
        let n=expected.iter().position(|&name|name==bytes).ok_or_else(||bad("unexpected entry in the selected product directory; not taking it over"))?;
        if seen&(1u16<<n)!=0{return Err(bad("duplicate entry in the dedicated instance inventory"));}
        seen|=1u16<<n;
    }
    if seen!=(1u16<<expected.len())-1||Times::on(&other)?!=before||Times::on(dir)?!=before{return Err(bad("incomplete or changed selected product directory"));}
    Ok(())
}
fn cancelled()->io::Result<()>{
    if helper_linux_evidence::diagnostic_process::cancelled(){Err(io::Error::new(io::ErrorKind::Interrupted,"offline issuance interrupted; preserve original state"))}
    else{Ok(())}
}
pub(in crate::sqlite_install::pair) fn sealed(bytes:&[u8])->io::Result<File>{
    use rustix::fs::{MemfdFlags,SealFlags,fcntl_add_seals,fcntl_get_seals,memfd_create};
    let mut f=File::from(memfd_create(c"helper-fixed-origin",MemfdFlags::CLOEXEC|MemfdFlags::ALLOW_SEALING|MemfdFlags::NOEXEC_SEAL)?);
    f.write_all(bytes)?;let seals=SealFlags::WRITE|SealFlags::GROW|SealFlags::SHRINK|SealFlags::SEAL;
    fcntl_add_seals(&f,seals)?;
    if !fcntl_get_seals(&f)?.contains(seals|SealFlags::EXEC)||f.metadata()?.len()!=bytes.len() as u64{return Err(bad("incomplete fixed bootstrap artifact"));}
    f.rewind()?;Ok(f)
}
