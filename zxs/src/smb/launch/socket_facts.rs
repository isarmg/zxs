//! Read only the original socket facts used by startup custody.
use super::{Fault,Result};
use std::{ffi::c_int,os::fd::{AsRawFd,BorrowedFd}};

// Indices into the native socket_integer_facts table.
#[repr(u8)]
pub(super) enum Integer {
    Domain=1, SocketType=2, Protocol=3, Accepting=4, ReusePort=5,
    PassCredentials=6, PassPidFd=7, V6Only=8,
}
#[cfg(target_arch="x86_64")]
#[repr(C)]
struct PeerCredentials {pid:i32,uid:u32,gid:u32}

unsafe extern "C"{
    fn observe_socket_integer(fd:c_int,kind:u8,out:*mut c_int)->c_int;
    fn observe_socket_cookie(fd:c_int,out:*mut u64)->c_int;
    fn observe_socket_unconnected(fd:c_int)->c_int;
    #[cfg(target_arch="x86_64")]
    fn observe_socket_peer(fd:c_int,out:*mut PeerCredentials)->c_int;
}

pub(super) fn integer(fd:BorrowedFd<'_>,kind:Integer)->Result<c_int>{
    let mut value=0;
    // SAFETY: the borrowed original and writable result live through one fixed getter.
    if unsafe{observe_socket_integer(fd.as_raw_fd(),kind as u8,&raw mut value)}!=0{return Err(Fault::Handoff);}
    Ok(value)
}
pub(super) fn cookie(fd:BorrowedFd<'_>)->Result<u64>{
    let mut value=0;
    // SAFETY: the borrowed original and writable result live through the cookie getter.
    if unsafe{observe_socket_cookie(fd.as_raw_fd(),&raw mut value)}!=0{return Err(Fault::Handoff);}
    Ok(value)
}
pub(super) fn require_unconnected(fd:BorrowedFd<'_>)->Result<()>{
    // SAFETY: the original borrowed socket lives through one fixed peer query.
    if unsafe{observe_socket_unconnected(fd.as_raw_fd())}!=0{return Err(Fault::Handoff);}
    Ok(())
}
#[cfg(target_arch="x86_64")]
pub(super) fn peer_credentials(fd:BorrowedFd<'_>)->Result<(i32,u32,u32)>{
    let mut peer=PeerCredentials{pid:0,uid:0,gid:0};
    // SAFETY: C-layout output and a live borrowed original; only the fixed peer query.
    if unsafe{observe_socket_peer(fd.as_raw_fd(),&raw mut peer)}!=0{return Err(Fault::Handoff);}
    Ok((peer.pid,peer.uid,peer.gid))
}
