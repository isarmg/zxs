//! Exact kernel evidence shared by the existing inherited role and the
//! proposed same-ELF parent. Neither a pidfd nor a channel grants a site,
//! a credential generation, or permission to listen.
use super::{Fault,Result};
use std::{
    fs::File,mem::{size_of,zeroed},
    os::fd::{AsRawFd,FromRawFd,OwnedFd,RawFd},os::unix::fs::MetadataExt,
};

const PID_FS_MAGIC:libc::c_long=0x50494446; // Linux v6.12 include/uapi/linux/magic.h

/// The identity is attached to this retained Linux object, never a PID in a
/// message or an independently reopened numeric PID.
pub(super) struct OriginalProcess{file:File,identity:(u64,u64)}
impl OriginalProcess{
    pub(super) fn pin(file:File)->Result<Self>{
        cloexec(file.as_raw_fd())?;
        let identity=pid_identity(&file)?;
        Ok(Self{file,identity})
    }
    pub(super) fn same_sender(&self,received:File)->Result<()>{
        let sent=Self::pin(received)?;
        if sent.identity!=self.identity{return Err(Fault::Handoff);}
        Ok(())
    }
    pub(super) fn poll_fd(&self)->libc::pollfd{
        libc::pollfd{fd:self.file.as_raw_fd(),events:libc::POLLIN,revents:0}
    }
    pub(super) fn require_live(&self)->Result<()>{
        let mut wait=self.poll_fd();
        // SAFETY: a single initialized poll record holding this live FD.
        if unsafe{libc::poll(&mut wait,1,0)}==0{Ok(())}else{Err(Fault::Handoff)}
    }
    #[cfg(target_arch="x86_64")]
    #[allow(dead_code)] // used only by the disabled high-side preparation
    pub(super) fn current()->Result<Self>{
        // SAFETY: Linux pidfd_open for the actual calling process group
        // leader; it cannot disappear while executing this syscall. No
        // caller PID is accepted, including for later child custody.
        let file=unsafe{File::from(new_fd(libc::syscall(libc::SYS_pidfd_open,libc::getpid(),0u32))?)};
        let this=Self::pin(file)?;
        this.require_live()?;Ok(this)
    }
}
#[cfg(target_arch="x86_64")]
impl AsRawFd for OriginalProcess{
    fn as_raw_fd(&self)->RawFd{self.file.as_raw_fd()}
}
fn pid_identity(file:&File)->Result<(u64,u64)>{
    if fs_magic(file.as_raw_fd())?!=PID_FS_MAGIC{return Err(Fault::Handoff);}
    let m=file.metadata().map_err(|_|Fault::Handoff)?;
    if m.ino()==0{return Err(Fault::Handoff);}
    Ok((m.dev(),m.ino()))
}

pub(super) struct BootSocket(OwnedFd);
impl BootSocket{
    pub(super) fn pin(fd:OwnedFd)->Result<Self>{
        cloexec(fd.as_raw_fd())?;
        for (opt,value) in [(libc::SO_DOMAIN,libc::AF_UNIX),(libc::SO_TYPE,libc::SOCK_SEQPACKET),
            (libc::SO_PASSCRED,1),(libc::SO_PASSPIDFD,1),(libc::SO_ACCEPTCONN,0)]{
            if sock_int(fd.as_raw_fd(),libc::SOL_SOCKET,opt)?!=value{return Err(Fault::Handoff);}
        }
        Ok(Self(fd))
    }
}
impl AsRawFd for BootSocket{
    fn as_raw_fd(&self)->RawFd{self.0.as_raw_fd()}
}
#[cfg(target_arch="x86_64")]
impl std::os::fd::AsFd for BootSocket{
    fn as_fd(&self)->std::os::fd::BorrowedFd<'_>{std::os::fd::AsFd::as_fd(&self.0)}
}

// The parent may create this pair, but the already-attested exact scoped
// release/site authority (a separate layer) must precede any START send.
#[cfg(target_arch="x86_64")]
#[allow(dead_code)]
pub(super) fn fresh_pair()->Result<(BootSocket,BootSocket)>{
    let mut fds=[-1;2];
    // SAFETY: only an unnamed local pair with two live writable int slots.
    let made=unsafe{libc::socketpair(libc::AF_UNIX,libc::SOCK_SEQPACKET|libc::SOCK_CLOEXEC,0,fds.as_mut_ptr())};
    if made!=0{return Err(Fault::Handoff);}
    // SAFETY: successful socketpair installs two distinct, owned descriptors.
    let [left,right]=fds.map(|fd|unsafe{OwnedFd::from_raw_fd(fd)});
    // SAFETY: read actual local credentials, not user-supplied claims.
    let own=unsafe{(libc::getpid(),libc::geteuid(),libc::getegid())};
    const _:()={assert!(size_of::<libc::ucred>()==12);};
    for fd in [&left,&right]{
        let fd=fd.as_raw_fd();
        let who=sock::<12>(fd,libc::SOL_SOCKET,libc::SO_PEERCRED)?;
        let (pid,uid,gid)=(i32::from_ne_bytes(who[..4].try_into().expect("four bytes")),
            u32::from_ne_bytes(who[4..8].try_into().expect("four bytes")),
            u32::from_ne_bytes(who[8..].try_into().expect("four bytes")));
        if (pid,uid,gid)!=own{return Err(Fault::Handoff);}
        for opt in [libc::SO_PASSCRED,libc::SO_PASSPIDFD]{
            if sock_int(fd,libc::SOL_SOCKET,opt)?!=0{return Err(Fault::Handoff);}
            let yes:libc::c_int=1;
            // SAFETY: fixed integer setting on this new private endpoint.
            if unsafe{libc::setsockopt(fd,libc::SOL_SOCKET,opt,(&raw const yes).cast(),size_of::<libc::c_int>() as _)}!=0{
                return Err(Fault::Handoff);
            }
        }
    }
    Ok((BootSocket::pin(left)?,BootSocket::pin(right)?))
}
pub(super) fn fs_magic(fd:RawFd)->Result<libc::c_long>{
    // SAFETY: a correctly laid out writable C output record.
    let mut buf:libc::statfs=unsafe{zeroed()};
    if unsafe{libc::fstatfs(fd,&mut buf)}!=0{return Err(Fault::Handoff);}
    Ok(buf.f_type as libc::c_long)
}
pub(super) fn get_flags(fd:RawFd,cmd:libc::c_int)->Result<libc::c_int>{
    if !matches!(cmd,libc::F_GETFD|libc::F_GETFL|libc::F_GET_SEALS){return Err(Fault::Handoff);}
    // SAFETY: these GET commands do not require a third fcntl argument.
    let v=unsafe{libc::fcntl(fd,cmd)};
    if v<0{Err(Fault::Handoff)}else{Ok(v)}
}
pub(super) fn cloexec(fd:RawFd)->Result<()>{
    if get_flags(fd,libc::F_GETFD)?&libc::FD_CLOEXEC==0{Err(Fault::Handoff)}else{Ok(())}
}
pub(super) fn sock<const N:usize>(fd:RawFd,layer:libc::c_int,typ:libc::c_int)->Result<[u8;N]>{
    let mut b=[0;N];let mut n=N as libc::socklen_t;
    // SAFETY: output pointer and its in/out length cover the live array.
    if unsafe{libc::getsockopt(fd,layer,typ,b.as_mut_ptr().cast(),&mut n)}!=0||n as usize!=N{
        return Err(Fault::Handoff);
    }
    Ok(b)
}
pub(super) fn sock_int(fd:RawFd,layer:libc::c_int,typ:libc::c_int)->Result<libc::c_int>{
    Ok(libc::c_int::from_ne_bytes(sock(fd,layer,typ)?))
}
/// Wrap only a fresh Linux descriptor just returned by the stated syscall.
///
/// # Safety
/// If nonnegative, ret must be an owned descriptor not otherwise managed.
#[cfg(target_arch="x86_64")]
pub(super) unsafe fn new_fd(ret:libc::c_long)->Result<OwnedFd>{
    if ret<0{return Err(Fault::Handoff);}
    let n=RawFd::try_from(ret).map_err(|_|Fault::Handoff)?;
    // SAFETY: ownership and a fresh kernel result are caller preconditions.
    Ok(unsafe{OwnedFd::from_raw_fd(n)})
}
