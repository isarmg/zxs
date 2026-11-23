//! Private foreground-terminal and inherited-descriptor evidence. No storage mutation.
pub(crate) mod channel;
pub(crate) use channel::PublicConfirmation;

use crate::{disk_web::require_terminal, pinned::Mount};
use rustix::fs::{self, Mode, OFlags, PROC_SUPER_MAGIC, RawDir, ResolveFlags, fstatfs, open, openat2};
use std::{fs::File, io, mem::MaybeUninit, os::fd::{AsFd, AsRawFd}};

fn denied()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"exact protected host administrator, explicit original scope and controlling terminal required")}
pub(crate) fn same_terminal(input:&io::Stdin,output:&io::Stderr)->io::Result<()>{
    let stdout=io::stdout();
    require_terminal(input.as_fd())?;require_terminal(output.as_fd())?;require_terminal(stdout.as_fd())?;
    let a=fs::fstat(input)?;
    for b in [fs::fstat(output)?,fs::fstat(stdout)?]{
        if a.st_dev!=b.st_dev||a.st_ino!=b.st_ino||a.st_rdev!=b.st_rdev{return Err(denied());}
    }Ok(())
}
fn own_descriptor_table()->io::Result<(File,File)>{
    let flags=OFlags::PATH|OFlags::DIRECTORY|OFlags::NOFOLLOW|OFlags::CLOEXEC;
    let proc=File::from(open(c"/proc",flags,Mode::empty())?);
    let mount=Mount::on(&proc)?.filter(|m|m.at_root==Some(true)).ok_or_else(denied)?;
    if fstatfs(&proc)?.f_type!=PROC_SUPER_MAGIC{return Err(denied());}
    let dir=File::from(openat2(&proc,c"thread-self/fd",OFlags::RDONLY|OFlags::DIRECTORY|OFlags::NOFOLLOW|OFlags::CLOEXEC,
        Mode::empty(),ResolveFlags::BENEATH|ResolveFlags::NO_XDEV|ResolveFlags::NO_MAGICLINKS)?);
    if Mount::on(&dir)?.is_none_or(|m|m.id!=mount.id)||fstatfs(&dir)?.f_type!=PROC_SUPER_MAGIC{return Err(denied());}
    Ok((proc,dir))
}
pub(crate) fn no_inherited_descriptors()->io::Result<()>{ only_inherited_descriptors(&[]) }
pub(crate) fn only_inherited_descriptors(already_verified:&[i32])->io::Result<()>{
    let (proc,dir)=own_descriptor_table()?;
    let mut buf=[MaybeUninit::uninit();4096];let mut entries=RawDir::new(&dir,&mut buf);
    while let Some(e)=entries.next(){
        let e=e?;let bytes=e.file_name().to_bytes();if bytes==b"."||bytes==b".."{continue;}
        let fd=std::str::from_utf8(bytes).ok().and_then(|s|s.parse::<i32>().ok()).ok_or_else(denied)?;
        if !matches!(fd,0..=2)&&fd!=proc.as_raw_fd()&&fd!=dir.as_raw_fd()&&!already_verified.contains(&fd){return Err(denied());}
    }
    Ok(())
}
