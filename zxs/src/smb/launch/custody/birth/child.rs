//! The fixed, allocation-free post-clone branch. No caller-selected command,
//! executable path, mountpoint, or environment is interpreted here.
use super::Role;
use crate::smb::launch::custody::view;
use std::{mem::zeroed,ptr};

#[repr(C)]
struct CapsHeader{version:u32,pid:i32}
#[repr(C)]
struct Caps{effective:u32,permitted:u32,inheritable:u32}

const fn stmt(code:u16,k:u32)->libc::sock_filter{libc::sock_filter{code,jt:0,jf:0,k}}
const fn j(code:u16,k:u32,yes:u8,no:u8)->libc::sock_filter{libc::sock_filter{code,jt:yes,jf:no,k}}
const NS:u32=(libc::CLONE_NEWUSER|libc::CLONE_NEWPID|libc::CLONE_NEWNS|libc::CLONE_NEWNET|
    libc::CLONE_NEWUTS|libc::CLONE_NEWIPC|libc::CLONE_NEWCGROUP)as u32|128;
const DENY:u32=0x0005_0000|libc::EPERM as u32;
// Same Linux x86-64 seccomp namespace/lifetime restriction as the qualified
// external parent. No new child namespace or namespace reentry once lowered.
static FILTER:[libc::sock_filter;17]=[
    stmt(0x20,4),j(0x15,0xc000_003e,1,0),stmt(0x06,0x8000_0000),
    stmt(0x20,0),j(0x35,0x4000_0000,0,1),stmt(0x06,0x8000_0000),
    j(0x15,libc::SYS_clone3 as u32,0,1),stmt(0x06,0x0005_0000|libc::ENOSYS as u32),
    j(0x15,libc::SYS_setns as u32,0,1),stmt(0x06,DENY),j(0x15,libc::SYS_unshare as u32,0,1),stmt(0x06,DENY),
    j(0x15,libc::SYS_clone as u32,0,3),stmt(0x20,16),j(0x45,NS,0,1),stmt(0x06,DENY),stmt(0x06,0x7fff_0000),
];
const fn z()->libc::c_ulong{0}
unsafe fn watch_parent(pidfd:i32)->bool{
    let mut p=libc::pollfd{fd:pidfd,events:libc::POLLIN,revents:0};
    // SAFETY: only this fresh child's death signal and an exact, retained
    // parent descriptor. Poll closes the race with a prior parent exit.
    unsafe{libc::prctl(libc::PR_SET_PDEATHSIG,libc::SIGKILL as libc::c_ulong,z(),z(),z())==0&&
        libc::poll(&raw mut p,1,0)==0}
}
unsafe fn limits(uid:u32,gid:u32)->bool{
    // SAFETY: no other thread and no shared tasks. All syscalls are fixed
    // and either further confine this child or make it fail immediately.
    unsafe{
        if libc::prctl(libc::PR_CAP_AMBIENT,libc::PR_CAP_AMBIENT_CLEAR_ALL as libc::c_ulong,z(),z(),z())!=0||
            libc::prctl(libc::PR_CAPBSET_READ,41 as libc::c_ulong,z(),z(),z())!=-1||
            *libc::__errno_location()!=libc::EINVAL{return false;}
        for bit in 0u32..=40{
            if libc::prctl(libc::PR_CAPBSET_DROP,bit as libc::c_ulong,z(),z(),z())!=0{return false;}
        }
        if libc::syscall(libc::SYS_setgroups,0usize,ptr::null::<libc::gid_t>())!=0||
            libc::syscall(libc::SYS_setresgid,gid,gid,gid)!=0||libc::syscall(libc::SYS_setresuid,uid,uid,uid)!=0{return false;}
        let h=CapsHeader{version:0x2008_0522,pid:0};
        let zero=Caps{effective:0,permitted:0,inheritable:0};
        let masks=[zero,Caps{effective:0,permitted:0,inheritable:0}];
        if libc::syscall(libc::SYS_capset,&raw const h,masks.as_ptr())!=0||
            libc::prctl(libc::PR_SET_NO_NEW_PRIVS,1 as libc::c_ulong,z(),z(),z())!=0{return false;}
        let bpf=libc::sock_fprog{len:FILTER.len() as u16,filter:FILTER.as_ptr().cast_mut()};
        libc::syscall(libc::SYS_seccomp,1u32,0u32,&raw const bpf)==0
    }
}
unsafe fn clean_signals()->bool{
    // SAFETY: reset only this freshly cloned task's separate dispositions.
    unsafe{
        let mut action:libc::sigaction=zeroed();action.sa_sigaction=libc::SIG_DFL;
        let mut empty:libc::sigset_t=zeroed();
        if libc::sigemptyset(&mut action.sa_mask)!=0||libc::sigemptyset(&mut empty)!=0{return false;}
        for sig in 1..=64{
            if matches!(sig,libc::SIGKILL|libc::SIGSTOP|32|33){continue;}
            if libc::sigaction(sig,&action,ptr::null_mut())!=0{return false;}
        }
        libc::sigprocmask(libc::SIG_SETMASK,&empty,ptr::null_mut())==0
    }
}

#[derive(Clone,Copy)]
pub(super) struct Inputs{
    pub(super) image:i32,pub(super) view:i32,pub(super) parent:i32,pub(super) peer:i32,
    pub(super) fail:i32,pub(super) null:i32,pub(super) role:Role,pub(super) uid:u32,pub(super) gid:u32,
}
/// Only the actual post-clone child, with its independent VM and descriptor
/// table. Return never touches parent code, a Rust destructor or allocator.
pub(super) unsafe fn start(i:Inputs)->!{
    // SAFETY: cloned task has only fixed high descriptors; all pointers
    // and C strings below have actual static or local lifetime.
    unsafe{
        let fail=||->!{libc::write(i.fail,b"!".as_ptr().cast(),1);libc::_exit(126)};
        if !watch_parent(i.parent)||!clean_signals()||libc::getpid()!=1||
            libc::syscall(libc::SYS_close_range,3u32,u32::MAX,libc::CLOSE_RANGE_CLOEXEC)!=0{fail();}
        let core=libc::rlimit{rlim_cur:0,rlim_max:0};
        if libc::setrlimit(libc::RLIMIT_CORE,&core)!=0||libc::prctl(libc::PR_SET_DUMPABLE,z(),z(),z(),z())!=0||
            !view::enter_child(i.view){fail();}
        let protected=libc::MS_RDONLY|libc::MS_NODEV|libc::MS_NOSUID|libc::MS_NOEXEC;
        if libc::mount(c"proc".as_ptr(),c"/proc".as_ptr(),c"proc".as_ptr(),protected,
            c"hidepid=2,subset=pid".as_ptr().cast())!=0||libc::setsid()<0{fail();}
        for target in 0..3{if libc::dup3(i.null,target,0)!=target{fail();}}
        if !limits(i.uid,i.gid)||!watch_parent(i.parent){fail();} // credential drop clears the death signal
        libc::umask(0o077);
        if libc::dup3(i.peer,3,0)!=3||libc::dup3(i.parent,4,0)!=4{fail();}
        let argv=[i.role.marker().as_ptr(),ptr::null()];
        let env=[ptr::null::<libc::c_char>()];
        libc::syscall(libc::SYS_execveat,i.image,c"".as_ptr(),argv.as_ptr(),env.as_ptr(),libc::AT_EMPTY_PATH);
        fail()
    }
}
