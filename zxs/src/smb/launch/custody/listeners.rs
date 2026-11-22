//! Own the single foreground IPv4 TCP/445 listener and its original FD.
use super::{Fault,Result,descriptor};
use super::super::DIRECT_BIND;
use crate::smb::launch::socket_facts::{self,Integer};
use std::{mem::size_of,net::TcpListener,os::fd::{AsFd,AsRawFd,BorrowedFd,FromRawFd,OwnedFd}};

const BACKLOG:i32=128;

pub(super) struct Listener{socket:TcpListener,cookie:u64}
impl Listener{
    pub(super) fn bind(check:impl Fn()->Result<()>)->Result<Self>{
        check()?;
        // SAFETY: creates only our original Linux TCP socket, CLOEXEC and NB.
        let raw=unsafe{libc::socket(libc::AF_INET,libc::SOCK_STREAM|libc::SOCK_CLOEXEC|libc::SOCK_NONBLOCK,libc::IPPROTO_TCP)};
        if raw<0{return Err(Fault::Handoff);}
        // SAFETY: the successful syscall yielded the owned descriptor.
        let fd=unsafe{OwnedFd::from_raw_fd(raw)};
        let reuse:libc::c_int=1;
        // SAFETY: only this owned socket and one initialized integer value.
        if unsafe{libc::setsockopt(raw,libc::SOL_SOCKET,libc::SO_REUSEADDR,(&raw const reuse).cast(),
            size_of::<libc::c_int>() as _)}!=0{return Err(Fault::Handoff);}
        let local=libc::sockaddr_in{sin_family:libc::AF_INET as _,sin_port:DIRECT_BIND.port().to_be(),
            sin_addr:libc::in_addr{s_addr:0},sin_zero:[0;8]};
        // SAFETY: the fixed IPv4 wildcard sockaddr is valid and retained here.
        if unsafe{libc::bind(raw,(&raw const local).cast(),size_of::<libc::sockaddr_in>() as _)}!=0{
            return Err(Fault::Handoff);
        }
        let cookie=socket_facts::cookie(fd.as_fd())?;
        let this=Self{socket:TcpListener::from(fd),cookie};
        this.check_bound(false)?;
        check()?;
        // SAFETY: this is our original bound TCP descriptor.
        if unsafe{libc::listen(this.socket.as_raw_fd(),BACKLOG)}!=0{return Err(Fault::Handoff);}
        this.require()?;
        check()?;
        Ok(this)
    }
    pub(super) fn cookie(&self)->u64{self.cookie}
    pub(super) fn descriptor(&self)->BorrowedFd<'_>{self.socket.as_fd()}
    pub(super) fn require(&self)->Result<()>{self.check_bound(true)}
    fn check_bound(&self,listening:bool)->Result<()>{
        let socket=self.socket.as_fd();let raw=socket.as_raw_fd();
        descriptor::cloexec(raw)?;
        if !descriptor::status(raw)?.nonblocking()||self.cookie==0||
            socket_facts::cookie(socket)?!=self.cookie||
            socket_facts::integer(socket,Integer::Domain)?!=libc::AF_INET||
            socket_facts::integer(socket,Integer::SocketType)?!=libc::SOCK_STREAM||
            socket_facts::integer(socket,Integer::Protocol)?!=libc::IPPROTO_TCP||
            socket_facts::integer(socket,Integer::ReusePort)?!=0||
            socket_facts::integer(socket,Integer::Accepting)?!=i32::from(listening)||
            self.socket.local_addr().map_err(|_|Fault::Handoff)?!=DIRECT_BIND{return Err(Fault::Handoff);}
        socket_facts::require_unconnected(socket)
    }
}
