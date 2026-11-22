//! Restrict a previously admitted private loopback browser worker.
#![allow(unsafe_code)]
use std::{fs::File,io,os::fd::{AsRawFd,FromRawFd}};
const HANDLED_FS:u64=(1<<16)-1; // Linux 6.12 ABI 6
#[repr(C)]struct RuleSet{fs:u64,net:u64}
fn no()->io::Error{io::Error::other("fixed local panel confinement not established")}

/// Close the already nonroot first-install page around its one fixed IPv4
/// listener and inherited anonymous request channel. No actual selections
/// or writable installation descriptors entered this process.
///
/// # Errors
/// Returns an error for an invalid listener or worker identity, insufficient
/// Landlock ABI, or failure to install the fixed Landlock and seccomp rules.
pub fn first_install_panel(listener:&std::net::TcpListener)->io::Result<()>{
    const KILL:u32=0x8000_0000;
    const ALLOW:u32=0x7fff_0000;
    const DENY:u32=0x0005_0000|(libc::EPERM as u32);
    let addr=listener.local_addr()?;
    if !matches!(addr.ip(),std::net::IpAddr::V4(ip) if ip==std::net::Ipv4Addr::LOCALHOST)
        ||addr.port()==0||listener.as_raw_fd()<4||rustix::process::geteuid().is_root(){
        return Err(no());
    }
    // Deny FS content/traversal and every new TCP bind/connect. The one
    // already-bound socket remains usable for accepting paired requests.
    if unsafe{libc::syscall(libc::SYS_landlock_create_ruleset,std::ptr::null::<RuleSet>(),0usize,1u32)}<6{
        return Err(no());
    }
    let attr=RuleSet{fs:HANDLED_FS,net:3};
    let raw=unsafe{libc::syscall(libc::SYS_landlock_create_ruleset,&raw const attr,std::mem::size_of::<RuleSet>(),0u32)};
    if raw<0{return Err(io::Error::last_os_error());}
    let fd=i32::try_from(raw).map_err(|_|no())?;
    let empty=unsafe{File::from_raw_fd(fd)};
    if unsafe{libc::syscall(libc::SYS_landlock_restrict_self,empty.as_raw_fd(),0u32)}!=0{
        return Err(io::Error::last_os_error());
    }
    drop(empty);
    let mut p=Vec::new();
    let mut b=|code,jt,jf,k|p.push(libc::sock_filter{code,jt,jf,k});
    b(0x20,0,0,4);b(0x15,1,0,0xc000_003e);b(0x06,0,0,KILL);
    b(0x20,0,0,0);b(0x35,0,1,0x4000_0000);b(0x06,0,0,KILL);
    // accept4 only of the actual pre-bound listener; no further socket(),
    // connect, bind, fork, fd-open, exec or process signaling syscall.
    b(0x15,0,6,u32::try_from(libc::SYS_accept4).map_err(|_|no())?);
    b(0x20,0,0,20);b(0x15,0,3,0);
    b(0x20,0,0,16);b(0x15,0,1,u32::try_from(listener.as_raw_fd()).map_err(|_|no())?);
    b(0x06,0,0,ALLOW);b(0x06,0,0,DENY);
    for syscall in [libc::SYS_read,libc::SYS_write,libc::SYS_readv,libc::SYS_writev,
        libc::SYS_recvfrom,libc::SYS_sendto,libc::SYS_recvmsg,libc::SYS_sendmsg,libc::SYS_close,
        libc::SYS_getsockname,libc::SYS_getpeername,libc::SYS_getsockopt,libc::SYS_setsockopt,libc::SYS_shutdown,
        libc::SYS_fcntl,libc::SYS_ioctl,libc::SYS_getrandom,libc::SYS_clock_gettime,libc::SYS_clock_nanosleep,
        libc::SYS_nanosleep,libc::SYS_getpid,libc::SYS_getppid,libc::SYS_brk,libc::SYS_mmap,libc::SYS_munmap,
        libc::SYS_mprotect,libc::SYS_mremap,libc::SYS_madvise,libc::SYS_futex,libc::SYS_sched_yield,
        libc::SYS_rt_sigreturn,libc::SYS_rt_sigprocmask,libc::SYS_sigaltstack,libc::SYS_exit,libc::SYS_exit_group]{
        b(0x15,0,1,u32::try_from(syscall).map_err(|_|no())?);b(0x06,0,0,ALLOW);
    }
    b(0x06,0,0,DENY);
    let prog=libc::sock_fprog{len:p.len().try_into().map_err(|_|no())?,filter:p.as_mut_ptr()};
    if unsafe{libc::syscall(libc::SYS_seccomp,1u32,0u32,&raw const prog)}!=0{return Err(io::Error::last_os_error());}
    Ok(())
}
