//! One irreversible policy in the dedicated, already single-threaded root
//! *offline account* process, after spawning its exact SQL worker but
//! before mapping the native TDB code or prompting for an SMB secret.
#![allow(unsafe_code)]
use std::{fs::File,io,os::fd::{AsRawFd,FromRawFd}};

const WRITE:u64=1<<1;
const READ:u64=1<<2;
const LIST:u64=1<<3;
const MAKE_REG:u64=1<<8;
const TRUNCATE:u64=1<<14;
const HANDLED_FS:u64=(1<<16)-1; // all filesystem access rights in Linux 6.12 ABI v6
#[repr(C)]struct RuleSet { fs:u64, net:u64 }
#[repr(C,packed)] struct Path { rights:u64, fd:i32 }

fn no()->io::Error{io::Error::other("fixed offline credential process confinement not established")}
fn add(set:&File,file:&File,rights:u64)->io::Result<()>{
    let attr=Path{rights,fd:file.as_raw_fd()};
    // SAFETY: sized Linux packed PATH_BENEATH record, existing regular or
    // directory pin already fixed by this dedicated authority.
    if unsafe{libc::syscall(libc::SYS_landlock_add_rule,set.as_raw_fd(),1u32,&raw const attr,0u32)}!=0{
        return Err(io::Error::last_os_error());
    }Ok(())
}

/// File bodies are accessible only at the listed original, already pinned
/// inputs. Ancestor directory listing is retained for original-name checks;
/// no read access to their file bodies follows from LIST. TDB can grow only
/// within the exact selected root-only account directory. No host writable
/// file or tty path is inherited aside from the known original prompt FDs.
pub fn apply(root:&File,account:&File,files:&[&File],metadata_dir:&File,child:&File)->io::Result<()>{
    if files.is_empty()||files.len()>150||child.as_raw_fd()<3{return Err(no());}
    // SAFETY: version query has a null attr and zero size. ABI < 6 is not
    // silently relaxed, nor is a system without enabled Landlock supported.
    if unsafe{libc::syscall(libc::SYS_landlock_create_ruleset,std::ptr::null::<RuleSet>(),0usize,1u32)}<6{
        return Err(no());
    }
    let attr=RuleSet{fs:HANDLED_FS,net:3};
    // SAFETY: size covers exactly the first two stable u64 UAPI members.
    let raw=unsafe{libc::syscall(libc::SYS_landlock_create_ruleset,&raw const attr,std::mem::size_of::<RuleSet>(),0u32)};
    if raw<0{return Err(io::Error::last_os_error());}
    // SAFETY: successful syscall returned a new owned CLOEXEC FD.
    let rules=unsafe{File::from_raw_fd(raw as i32)};
    add(&rules,root,LIST)?;
    add(&rules,metadata_dir,LIST|READ)?; // own proc fdinfo only, not /proc or another process
    add(&rules,account,READ|LIST|WRITE|MAKE_REG|TRUNCATE)?;
    for f in files{
        let m=f.metadata()?;
        if !m.is_file(){return Err(no());}
        add(&rules,f,READ)?; // not general exec; no writable input
    }
    // SAFETY: no_new_privs irreversible for this one-shot process.
    if unsafe{libc::prctl(libc::PR_SET_NO_NEW_PRIVS,1 as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong)}!=0
        ||unsafe{libc::syscall(libc::SYS_landlock_restrict_self,rules.as_raw_fd(),0u32)}!=0{
        return Err(io::Error::last_os_error());
    }
    drop(rules);
    filter(child.as_raw_fd())
}
fn filter(pidfd:i32)->io::Result<()>{
    const KILL:u32=0x8000_0000; const ALLOW:u32=0x7fff_0000;const DENY:u32=0x0005_0000|(libc::EPERM as u32);
    // x86-64 classic BPF. Reject alternate syscall ABIs; no subprocess,
    // fresh socket, namespace alteration, ptrace/data injection, hard command
    // dispatch or rewrite of permissions. The only post-seal process signal
    // is through the held true SQLite child pidfd (never a PID from a message).
    let mut p=Vec::new();
    let mut emit=|code,jt,jf,k|p.push(libc::sock_filter{code,jt,jf,k});
    emit(0x20,0,0,4);emit(0x15,1,0,0xc000_003e);emit(0x06,0,0,KILL);
    emit(0x20,0,0,0);emit(0x35,0,1,0x4000_0000);emit(0x06,0,0,KILL);
    for name in [
        libc::SYS_execve,libc::SYS_execveat,libc::SYS_clone,libc::SYS_clone3,libc::SYS_fork,libc::SYS_vfork,libc::SYS_setsid,libc::SYS_setpgid,
        libc::SYS_socket,libc::SYS_socketpair,libc::SYS_connect,libc::SYS_bind,libc::SYS_listen,
        libc::SYS_accept,libc::SYS_accept4,libc::SYS_mount,libc::SYS_umount2,libc::SYS_open_tree,
        libc::SYS_move_mount,libc::SYS_mount_setattr,libc::SYS_fsopen,libc::SYS_fsmount,libc::SYS_fsconfig,libc::SYS_fspick,
        libc::SYS_setns,libc::SYS_unshare,libc::SYS_pivot_root,libc::SYS_chroot,libc::SYS_ptrace,
        libc::SYS_process_vm_readv,libc::SYS_process_vm_writev,libc::SYS_pidfd_getfd,libc::SYS_pidfd_open,
        libc::SYS_bpf,libc::SYS_userfaultfd,libc::SYS_io_uring_setup,libc::SYS_io_uring_enter,libc::SYS_io_uring_register,
        libc::SYS_setgid,libc::SYS_setuid,libc::SYS_setgroups,libc::SYS_setresuid,libc::SYS_setresgid,
        libc::SYS_setreuid,libc::SYS_setregid,libc::SYS_setfsuid,libc::SYS_setfsgid,libc::SYS_capset,
        libc::SYS_kill,libc::SYS_tkill,libc::SYS_tgkill,libc::SYS_rt_sigqueueinfo,libc::SYS_rt_tgsigqueueinfo,
        libc::SYS_chmod,libc::SYS_fchmod,libc::SYS_fchmodat,libc::SYS_fchmodat2,libc::SYS_chown,
        libc::SYS_fchown,libc::SYS_lchown,libc::SYS_fchownat,libc::SYS_setxattr,libc::SYS_lsetxattr,
        libc::SYS_fsetxattr,libc::SYS_removexattr,libc::SYS_lremovexattr,libc::SYS_fremovexattr,
        libc::SYS_keyctl,libc::SYS_request_key,libc::SYS_add_key,libc::SYS_perf_event_open,libc::SYS_reboot,libc::SYS_init_module,libc::SYS_finit_module,libc::SYS_delete_module,
        libc::SYS_open_by_handle_at,libc::SYS_memfd_create,
    ]{emit(0x15,0,1,name as u32);emit(0x06,0,0,DENY);}
    emit(0x15,0,7,libc::SYS_pidfd_send_signal as u32);
    emit(0x20,0,0,16);emit(0x15,0,4,pidfd as u32);
    emit(0x20,0,0,24);emit(0x15,1,0,libc::SIGTERM as u32);emit(0x15,0,1,libc::SIGKILL as u32);
    emit(0x06,0,0,ALLOW);emit(0x06,0,0,DENY);emit(0x06,0,0,ALLOW);
    let prog=libc::sock_fprog{len:p.len().try_into().map_err(|_|no())?,filter:p.as_mut_ptr()};
    // SAFETY: contiguous classic BPF copied by the kernel synchronously.
    if unsafe{libc::syscall(libc::SYS_seccomp,1u32,0u32,&raw const prog)}!=0{return Err(io::Error::last_os_error());}
    Ok(())
}

/// Close the already nonroot first-install page around its one fixed IPv4
/// listener and inherited anonymous request channel. No actual selections
/// or writable installation descriptors entered this process.
pub fn first_install_panel(listener:&std::net::TcpListener)->io::Result<()>{
    let addr=listener.local_addr()?;
    if !matches!(addr.ip(),std::net::IpAddr::V4(ip) if ip==std::net::Ipv4Addr::LOCALHOST)
        ||addr.port()==0||listener.as_raw_fd()<4||rustix::process::geteuid().is_root(){
        return Err(no());
    }
    // Same enrolled Linux ABI as the existing native maintenance boundary:
    // deny FS content/traversal and all *new* TCP bind/connect. The one
    // already-bound socket remains usable for accepting paired requests.
    if unsafe{libc::syscall(libc::SYS_landlock_create_ruleset,std::ptr::null::<RuleSet>(),0usize,1u32)}<6{
        return Err(no());
    }
    let attr=RuleSet{fs:HANDLED_FS,net:3};
    let raw=unsafe{libc::syscall(libc::SYS_landlock_create_ruleset,&raw const attr,std::mem::size_of::<RuleSet>(),0u32)};
    if raw<0{return Err(io::Error::last_os_error());}
    let empty=unsafe{File::from_raw_fd(raw as i32)};
    if unsafe{libc::syscall(libc::SYS_landlock_restrict_self,empty.as_raw_fd(),0u32)}!=0{
        return Err(io::Error::last_os_error());
    }
    drop(empty);
    let mut p=Vec::new();
    let mut b=|code,jt,jf,k|p.push(libc::sock_filter{code,jt,jf,k});
    const KILL:u32=0x8000_0000;const ALLOW:u32=0x7fff_0000;const DENY:u32=0x0005_0000|(libc::EPERM as u32);
    b(0x20,0,0,4);b(0x15,1,0,0xc000_003e);b(0x06,0,0,KILL);
    b(0x20,0,0,0);b(0x35,0,1,0x4000_0000);b(0x06,0,0,KILL);
    // accept4 only of the actual pre-bound listener; no further socket(),
    // connect, bind, fork, fd-open, exec or process signaling syscall.
    b(0x15,0,6,libc::SYS_accept4 as u32);
    b(0x20,0,0,20);b(0x15,0,3,0);
    b(0x20,0,0,16);b(0x15,0,1,listener.as_raw_fd() as u32);
    b(0x06,0,0,ALLOW);b(0x06,0,0,DENY);
    for syscall in [libc::SYS_read,libc::SYS_write,libc::SYS_readv,libc::SYS_writev,
        libc::SYS_recvfrom,libc::SYS_sendto,libc::SYS_recvmsg,libc::SYS_sendmsg,libc::SYS_close,
        libc::SYS_getsockname,libc::SYS_getpeername,libc::SYS_getsockopt,libc::SYS_setsockopt,libc::SYS_shutdown,
        libc::SYS_fcntl,libc::SYS_ioctl,libc::SYS_getrandom,libc::SYS_clock_gettime,libc::SYS_clock_nanosleep,
        libc::SYS_nanosleep,libc::SYS_getpid,libc::SYS_getppid,libc::SYS_brk,libc::SYS_mmap,libc::SYS_munmap,
        libc::SYS_mprotect,libc::SYS_mremap,libc::SYS_madvise,libc::SYS_futex,libc::SYS_sched_yield,
        libc::SYS_rt_sigreturn,libc::SYS_rt_sigprocmask,libc::SYS_sigaltstack,libc::SYS_exit,libc::SYS_exit_group]{
        b(0x15,0,1,syscall as u32);b(0x06,0,0,ALLOW);
    }
    b(0x06,0,0,DENY);
    let prog=libc::sock_fprog{len:p.len().try_into().map_err(|_|no())?,filter:p.as_mut_ptr()};
    if unsafe{libc::syscall(libc::SYS_seccomp,1u32,0u32,&raw const prog)}!=0{return Err(io::Error::last_os_error());}
    Ok(())
}
