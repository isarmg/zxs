//! Exact kernel evidence shared by the existing inherited role and the
//! proposed same-ELF parent. Neither a pidfd nor a channel grants a site,
//! a credential generation, or permission to listen.
use super::{Fault,Result,descriptor::{cloexec,filesystem_type},
    socket_facts::{self,Integer}};
use crate::smb::readiness::{self,Interest,Watch};
use std::{
    fs::File,mem::size_of,
    os::fd::{AsFd,AsRawFd,BorrowedFd,FromRawFd,OwnedFd,RawFd},os::unix::fs::MetadataExt,
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
    pub(super) fn poll_watch(&self)->Watch<'_>{Watch::new(self.file.as_fd(),Interest::Read)}
    pub(super) fn require_live(&self)->Result<()>{
        if readiness::quiet_now(self.file.as_fd()){Ok(())}else{Err(Fault::Handoff)}
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
        for (kind,value) in [(Integer::Domain,libc::AF_UNIX),(Integer::SocketType,libc::SOCK_SEQPACKET),
            (Integer::PassCredentials,1),(Integer::PassPidFd,1),(Integer::Accepting,0)]{
            if socket_facts::integer(fd.as_fd(),kind)?!=value{return Err(Fault::Handoff);}
        }
        Ok(Self(fd))
    }
}
impl AsRawFd for BootSocket{
    fn as_raw_fd(&self)->RawFd{self.0.as_raw_fd()}
}
impl AsFd for BootSocket{
    fn as_fd(&self)->BorrowedFd<'_>{self.0.as_fd()}
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
    for socket in [&left,&right]{
        let fd=socket.as_raw_fd();
        if socket_facts::peer_credentials(socket.as_fd())?!=own{return Err(Fault::Handoff);}
        for (kind,opt) in [(Integer::PassCredentials,libc::SO_PASSCRED),(Integer::PassPidFd,libc::SO_PASSPIDFD)]{
            if socket_facts::integer(socket.as_fd(),kind)?!=0{return Err(Fault::Handoff);}
            let yes:libc::c_int=1;
            // SAFETY: fixed integer setting on this new private endpoint.
            if unsafe{libc::setsockopt(fd,libc::SOL_SOCKET,opt,(&raw const yes).cast(),size_of::<libc::c_int>() as _)}!=0{
                return Err(Fault::Handoff);
            }
        }
    }
    Ok((BootSocket::pin(left)?,BootSocket::pin(right)?))
}
pub(super) fn fs_magic(fd:RawFd)->Result<std::ffi::c_long>{
    filesystem_type(fd).ok_or(Fault::Handoff)
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
