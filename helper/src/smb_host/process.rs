//! Inspect only our own new child, held by the supervising caller. No PID lookup
//! creates a pidfd, no wait reaps, no namespace is entered and no signal is sent.
use super::{no,same};
use crate::pinned::{Entity,Mount};
use rustix::{fs::{Mode,OFlags,PROC_SUPER_MAGIC,RawDir,ResolveFlags,fcntl_getfl,fstatfs,open,openat,openat2,
    readlinkat,readlinkat_raw},io::Errno,process::{WaitId,WaitIdOptions,waitid}};
use std::{ffi::{CStr,CString},fs::File,io::{self,Read},mem::MaybeUninit,os::{fd::{AsFd,AsRawFd},unix::fs::MetadataExt}};

const DIR:OFlags=OFlags::PATH.union(OFlags::DIRECTORY).union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);
const SAFE:ResolveFlags=ResolveFlags::BENEATH.union(ResolveFlags::NO_XDEV).union(ResolveFlags::NO_MAGICLINKS);

pub(super) fn waitable_live(birth:&File)->io::Result<()>{
    if fstatfs(birth)?.f_type as u64!=0x50494446 ||
        fcntl_getfl(birth)?.intersects(OFlags::PATH|OFlags::EXCL|OFlags::NONBLOCK){return Err(no());}
    // P_PIDFD + WNOWAIT: a nonchild is not a valid subject; zero available
    // exit events is not an assertion that the child is serving requests.
    loop{match waitid(WaitId::PidFd(birth.as_fd()),WaitIdOptions::NOWAIT|WaitIdOptions::NOHANG|WaitIdOptions::EXITED){
        Ok(None)=>return Ok(()),Ok(Some(_))=>return Err(no()),Err(Errno::INTR)=>continue,Err(e)=>return Err(e.into())
    }}
}
fn data(f:File,max:u64)->io::Result<Vec<u8>>{
    let mut out=Vec::new();f.take(max+1).read_to_end(&mut out)?;
    if out.len() as u64>max||!out.ends_with(b"\n"){return Err(no());}Ok(out)
}
fn field<'a>(text:&'a [u8],key:&[u8])->io::Result<Vec<&'a [u8]>>{
    let mut rows=text.split(|b|*b==b'\n').filter_map(|l|l.strip_prefix(key));
    let line=rows.next().ok_or_else(no)?;
    if rows.next().is_some()||line.first().is_none_or(|b|!b.is_ascii_whitespace()){return Err(no());}
    Ok(line.split(u8::is_ascii_whitespace).filter(|x|!x.is_empty()).collect())
}
fn pid(text:&[u8])->io::Result<u32>{
    if text.is_empty()||text.len()>10||text[0]==b'0'||!text.iter().all(u8::is_ascii_digit){return Err(no());}
    std::str::from_utf8(text).ok().and_then(|s|s.parse::<u32>().ok()).filter(|p|*p>1&&*p<=i32::MAX as u32).ok_or_else(no)
}
fn boot_pid(info:&[u8])->io::Result<u32>{
    let id=field(info,b"Pid:")?;
    if id.len()!=1||field(info,b"NSpid:")?.as_slice()!=[id[0],b"1"]{return Err(no());}
    pid(id[0])
}
fn status(bytes:&[u8],id:u32,parent:u32)->io::Result<()>{
    let own=id.to_string();let parent=parent.to_string();
    if field(bytes,b"Tgid:")?.as_slice()!=[own.as_bytes()]||field(bytes,b"Pid:")?.as_slice()!=[own.as_bytes()]
        ||field(bytes,b"PPid:")?.as_slice()!=[parent.as_bytes()]||field(bytes,b"NSpid:")?.as_slice()!=[own.as_bytes(),b"1"]
        ||field(bytes,b"Uid:")?.as_slice()!=[b"0".as_slice();4]||field(bytes,b"NoNewPrivs:")?.as_slice()!=[b"1"]{return Err(no());}
    for key in [b"CapInh:",b"CapPrm:",b"CapEff:",b"CapBnd:",b"CapAmb:"]{
        let v=field(bytes,key)?;
        if v.len()!=1||v[0].len()!=16||!v[0].iter().all(u8::is_ascii_hexdigit)
            ||u64::from_str_radix(std::str::from_utf8(v[0]).map_err(|_|no())?,16).map_err(|_|no())?&(1<<21)!=0{return Err(no());}
    }
    Ok(())
}
fn symlink(dir:&File,name:&CStr)->io::Result<File>{
    let l=File::from(openat2(dir,name,OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty(),SAFE)?);
    if !l.metadata()?.file_type().is_symlink()||fstatfs(&l)?.f_type!=PROC_SUPER_MAGIC{return Err(no());}
    Ok(l)
}
fn target(dir:&File,name:&CStr)->io::Result<File>{
    let _link=symlink(dir,name)?;
    Ok(File::from(openat(dir,name,OFlags::PATH|OFlags::CLOEXEC,Mode::empty())?))
}
fn pid_namespaces(birth:&File)->io::Result<()>{
    let [active,future,user]=helper_linux_evidence::smb_process::process_namespaces(birth)?;
    let owner=helper_linux_evidence::smb_process::pid_namespace_owner(&active)?;
    // Fixed Debian Linux baseline. Only one child PID namespace under the host
    // is supported. A user namespace around a lookalike PID 1 is not a host.
    if !same(&active,&future)?||active.metadata()?.ino()==0xefff_fffc
        ||user.metadata()?.ino()!=0xefff_fffd||!same(&owner,&user)?{return Err(no());}
    Ok(())
}
fn fd_number(bytes:&[u8],key:&[u8],radix:u32)->io::Result<u64>{
    let values=field(bytes,key)?;let [v]=values.as_slice() else{return Err(no());};
    if v.len()>22||!v.iter().all(|c|if radix==8{matches!(*c,b'0'..=b'7')}else{c.is_ascii_digit()}){return Err(no());}
    u64::from_str_radix(std::str::from_utf8(v).map_err(|_|no())?,radix).map_err(|_|no())
}
fn communicating(process:&File,fd_dir:&File,n:&CStr,daemon:&File)->io::Result<()>{
    // readlink and stat alone admit an O_PATH alias of the very same socket.
    // Only for the selected socket: Linux proc fdinfo samples this child's real
    // open-file flags, mount and inode together; no communication fd is copied.
    if !same(&target(fd_dir,n)?,daemon)?{return Err(no());}
    let infos=File::from(openat2(process,c"fdinfo",DIR,Mode::empty(),SAFE)?);
    let actual=File::from(openat2(&infos,n,OFlags::RDONLY|OFlags::NONBLOCK|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty(),SAFE)?);
    let info=data(actual,4096)?;
    let flags=fd_number(&info,b"flags:",8)?;
    if flags&3!=2 || flags&u64::from((OFlags::PATH|OFlags::NONBLOCK).bits())!=0
        ||fd_number(&info,b"mnt_id:",10)?!=Mount::on(daemon)?.ok_or_else(no)?.id
        ||fd_number(&info,b"ino:",10)?!=Entity::of(&daemon.metadata()?)?.ino
        ||!same(&target(fd_dir,n)?,daemon)?{return Err(no());}
    Ok(())
}
fn endpoint_set(process:&File,daemon:&File,storage:&File)->io::Result<()>{
    // Samba first receives 3, duplicates at >=5 and closes 3. Do not assume
    // that 3 remains open. Examine ONLY this original child's bounded proc
    // fd links. Other descriptors are never data-opened, copied, read or logged.
    let dir=File::from(openat2(process,c"fd",OFlags::RDONLY|OFlags::DIRECTORY|OFlags::CLOEXEC|OFlags::NOFOLLOW,Mode::empty(),SAFE)?);
    let label=|f:&File|->io::Result<String>{Ok(format!("socket:[{}]",Entity::of(&f.metadata()?)?.ino))};
    let want=label(daemon)?;let wrong=label(storage)?;
    let mut buf=[MaybeUninit::uninit();4096];let mut it=RawDir::new(&dir,&mut buf);
    let (mut seen,mut found)=(0u16,false);
    while let Some(entry)=it.next(){
        let e=entry?;let n=e.file_name();if n==c"."||n==c".."{continue;}
        seen+=1;if seen>256{return Err(no());}
        let digits=n.to_bytes();if digits.is_empty()||digits.len()>10||!digits.iter().all(u8::is_ascii_digit){return Err(no());}
        let link=symlink(&dir,n)?;let mut name=[0u8;64];
        let len=readlinkat_raw(&link,c"",&mut name[..])?;
        let label=&name[..len];if label!=want.as_bytes() && label!=wrong.as_bytes(){continue;}
        if label==wrong.as_bytes(){return Err(no());}
        let number=std::str::from_utf8(digits).ok().and_then(|v|v.parse::<u32>().ok()).ok_or_else(no)?;
        if number!=3&&number<5{return Err(no());}
        communicating(process,&dir,n,daemon)?;found=true;
    }
    if !found{return Err(no());}Ok(())
}

fn cap_mask(bytes:&[u8],key:&[u8])->io::Result<u64>{
    let vals=field(bytes,key)?;let [v]=vals.as_slice()else{return Err(no());};
    if v.len()!=16||!v.iter().all(u8::is_ascii_hexdigit){return Err(no());}
    u64::from_str_radix(std::str::from_utf8(v).map_err(|_|no())?,16).map_err(|_|no())
}
const SERVICE_CAPS:u64=(1<<0)|(1<<1)|(1<<3)|(1<<6)|(1<<7)|(1<<10);
const SUPERVISOR_CAPS:u64=SERVICE_CAPS|(1<<5)|(1<<8)|(1<<18)|(1<<19)|(1<<21);
fn supervisor_caps(bytes:&[u8])->io::Result<()>{
    // KILL is necessary once the storage child no longer has UID 0. PTRACE
    // is necessary to inspect that child's exe/ns. No emergency elevation.
    if cap_mask(bytes,b"CapEff:")?&SUPERVISOR_CAPS!=SUPERVISOR_CAPS||
       cap_mask(bytes,b"CapBnd:")?&SERVICE_CAPS!=SERVICE_CAPS{return Err(no());}Ok(())
}
pub(super) fn host()->io::Result<File>{
    let p=File::from(open(c"/proc",DIR,Mode::empty())?);
    if fstatfs(&p)?.f_type!=PROC_SUPER_MAGIC || Mount::on(&p)?.is_none_or(|m|m.at_root!=Some(true))
        ||readlinkat(&p,c"self",Vec::new())?.as_bytes()!=std::process::id().to_string().as_bytes(){return Err(no());}
    let current=File::from(openat2(&p,c"thread-self/ns",DIR,Mode::empty(),SAFE)?);
    let own_pid=target(&current,c"pid")?;let own_user=target(&current,c"user")?;
    if fstatfs(&own_pid)?.f_type as u64!=0x6e736673||own_pid.metadata()?.ino()!=0xefff_fffc
        ||fstatfs(&own_user)?.f_type as u64!=0x6e736673||own_user.metadata()?.ino()!=0xefff_fffd{return Err(no());}
    // A service supervisor, not a thread inside a loaded Web/SQL process.
    let t=File::from(openat2(&p,c"thread-self/status",OFlags::RDONLY|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty(),SAFE)?);
    let fields=data(t,65536)?;supervisor_caps(&fields)?;
    if field(&fields,b"Threads:")?.as_slice()!=[b"1".as_slice()]||
        field(&fields,b"Uid:")?.as_slice()!=[b"0".as_slice();4]||field(&fields,b"Gid:")?.as_slice()!=[b"0".as_slice();4]{return Err(no());}
    Ok(p)
}
fn active_dir(birth:&File)->io::Result<(u32,File)>{
    waitable_live(birth)?;
    // Route to genuine procfs only AFTER the original live pidfd was obtained.
    let name=CString::new(format!("thread-self/fdinfo/{}",birth.as_raw_fd())).expect("decimal descriptor");
    let id=boot_pid(&data(crate::maintenance::thread_file(&name)?,4096)?)?;
    let p=host()?;let idname=id.to_string();
    let original=File::from(openat2(&p,idname.as_str(),DIR,Mode::empty(),SAFE)?);
    waitable_live(birth)?;Ok((id,original))
}
pub(super) fn storage_snapshot(birth:&File,image:&File,uid:u32,gid:u32)->io::Result<bool>{
    let(id,original)=active_dir(birth)?;pid_namespaces(birth)?;
    if !same(&target(&original,c"exe")?,image)?{return Err(no());}
    let file=File::from(openat2(&original,c"status",OFlags::RDONLY|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty(),SAFE)?);
    let b=data(file,65536)?;let id=id.to_string();let parent=std::process::id().to_string();
    let(uid,gid)=(uid.to_string(),gid.to_string());
    if field(&b,b"Tgid:")?.as_slice()!=[id.as_bytes()]||field(&b,b"Pid:")?.as_slice()!=[id.as_bytes()]||
        field(&b,b"PPid:")?.as_slice()!=[parent.as_bytes()]||field(&b,b"NSpid:")?.as_slice()!=[id.as_bytes(),b"1"]||
        field(&b,b"Uid:")?.as_slice()!=[uid.as_bytes();4]||field(&b,b"Gid:")?.as_slice()!=[gid.as_bytes();4]||
        !field(&b,b"Groups:")?.is_empty()||field(&b,b"NoNewPrivs:")?.as_slice()!=[b"1".as_slice()]{return Err(no());}
    for k in [b"CapInh:",b"CapPrm:",b"CapEff:",b"CapBnd:",b"CapAmb:"]{
        if field(&b,k)?.as_slice()!=[b"0000000000000000".as_slice()]{return Err(no());}
    }
    let term=1u64<<(15-1);let handles_term=cap_mask(&b,b"SigCgt:")?&term!=0&&cap_mask(&b,b"SigBlk:")?&term==0;
    if !same(&target(&original,c"exe")?,image)?{return Err(no());}waitable_live(birth)?;Ok(handles_term)
}

pub(super) fn qualify(birth:&File,image:&File,daemon:&File,storage:&File)->io::Result<()>{
    let(id,original)=active_dir(birth)?;
    let snapshot=||->io::Result<()>{
        let s=File::from(openat2(&original,c"status",OFlags::RDONLY|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty(),SAFE)?);
        status(&data(s,65536)?,id,std::process::id())?;
        pid_namespaces(birth)?;
        if !same(&target(&original,c"exe")?,image)?{return Err(no());}
        Ok(())
    };
    snapshot()?;endpoint_set(&original,daemon,storage)?;snapshot()?;waitable_live(birth)
}

#[cfg(test)]
#[path="../../tests/smb-host/process.rs"]
mod tests;
