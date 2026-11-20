//! Fixed post-clone setup. No allocation, NSS, spawn, arbitrary paths or commands.
//! These mounts and credential changes affect only the not-yet-exec'd child.
use linux_raw_sys::general as u;
use std::{ffi::CStr,io,mem,ptr};

#[allow(unsafe_code)]
pub(super) fn verified_null(fd:i32)->io::Result<()>{
    let mut s:libc::stat=unsafe{mem::zeroed()};
    let f=unsafe{libc::fcntl(fd,libc::F_GETFL)};
    if f<0||unsafe{libc::fstat(fd,&raw mut s)}!=0||s.st_mode&libc::S_IFMT!=libc::S_IFCHR||
        libc::major(s.st_rdev)!=1||libc::minor(s.st_rdev)!=3||f&libc::O_PATH!=0||f&3!=libc::O_RDWR{
        return Err(io::Error::other("dedicated service requires the verified real null descriptor"));
    }
    Ok(())
}

// Basic lifetime restriction, not a security decision about users. The trusted
// service may fork and use threads but no fd3 holder can enter/create a new
// user/PID namespace and regain authority over sender credentials there.
const fn stmt(code:u16,k:u32)->libc::sock_filter{libc::sock_filter{code,jt:0,jf:0,k}}
const fn j(code:u16,k:u32,yes:u8,no:u8)->libc::sock_filter{libc::sock_filter{code,jt:yes,jf:no,k}}
const NS:u32=(libc::CLONE_NEWUSER|libc::CLONE_NEWPID|libc::CLONE_NEWNS|libc::CLONE_NEWNET|
    libc::CLONE_NEWUTS|libc::CLONE_NEWIPC|libc::CLONE_NEWCGROUP)as u32|128;
const DENY:u32=0x0005_0000|libc::EPERM as u32;
static FILTER:[libc::sock_filter;17]=[
    stmt(0x20,4),j(0x15,0xc000_003e,1,0),stmt(0x06,0x8000_0000),
    stmt(0x20,0),j(0x35,0x4000_0000,0,1),stmt(0x06,0x8000_0000),
    j(0x15,u::__NR_clone3,0,1),stmt(0x06,0x0005_0000|libc::ENOSYS as u32),
    j(0x15,u::__NR_setns,0,1),stmt(0x06,DENY),j(0x15,u::__NR_unshare,0,1),stmt(0x06,DENY),
    j(0x15,u::__NR_clone,0,3),stmt(0x20,16),j(0x45,NS,0,1),stmt(0x06,DENY),stmt(0x06,0x7fff_0000),
];

#[allow(unsafe_code)]
fn parent_live(parent:i32)->bool{unsafe{
    let mut p=libc::pollfd{fd:parent,events:libc::POLLIN,revents:0};
    libc::prctl(libc::PR_SET_PDEATHSIG,libc::SIGKILL as libc::c_ulong,zero(),zero(),zero())==0&&libc::poll(&raw mut p,1,0)==0
}}
const fn zero()->libc::c_ulong{0}
#[allow(unsafe_code)]
fn mount_child(path:&CStr,kind:&CStr,flags:libc::c_ulong,options:&CStr)->bool{unsafe{
    libc::mount(kind.as_ptr(),path.as_ptr(),kind.as_ptr(),flags,options.as_ptr().cast())==0
}}
#[allow(unsafe_code)]
fn install_limits(id:Option<(u32,u32)>)->bool{unsafe{
    let keep:u32=if id.is_some(){0}else{
        // No SYS_ADMIN, CHROOT, PTRACE, module, networking administration,
        // file capabilities or SETPCAP survive into the dedicated smbd.
        (1<<u::CAP_CHOWN)|(1<<u::CAP_DAC_OVERRIDE)|(1<<u::CAP_FOWNER)|
        (1<<u::CAP_SETUID)|(1<<u::CAP_SETGID)|(1<<u::CAP_NET_BIND_SERVICE)
    };
    if libc::prctl(libc::PR_CAP_AMBIENT,libc::PR_CAP_AMBIENT_CLEAR_ALL as libc::c_ulong,zero(),zero(),zero())!=0{return false;}
    // Exact supported Linux 6.12 capability vocabulary (0..40). Refuse a
    // newer unknown capability rather than leave it in a privileged child.
    if libc::prctl(libc::PR_CAPBSET_READ,41 as libc::c_ulong,zero(),zero(),zero())!=-1||*libc::__errno_location()!=libc::EINVAL{return false;}
    for bit in 0u32..=40{
        if u64::from(keep)&(1u64<<bit)==0 && libc::prctl(libc::PR_CAPBSET_DROP,bit as libc::c_ulong,zero(),zero(),zero())!=0{return false;}
    }
    if libc::syscall(libc::SYS_setgroups,0usize,ptr::null::<libc::gid_t>())!=0{return false;}
    if let Some((uid,gid))=id{
        if libc::syscall(libc::SYS_setresgid,gid,gid,gid)!=0||libc::syscall(libc::SYS_setresuid,uid,uid,uid)!=0{return false;}
    }else if libc::getuid()!=0||libc::geteuid()!=0||libc::getgid()!=0||libc::getegid()!=0{return false;}
    let hdr=u::__user_cap_header_struct{version:0x2008_0522,pid:0};
    let masks=[u::__user_cap_data_struct{effective:keep,permitted:keep,inheritable:0},
        u::__user_cap_data_struct{effective:0,permitted:0,inheritable:0}];
    if libc::syscall(libc::SYS_capset,&raw const hdr,masks.as_ptr())!=0||
        libc::prctl(libc::PR_SET_NO_NEW_PRIVS,1 as libc::c_ulong,zero(),zero(),zero())!=0{return false;}
    let bpf=libc::sock_fprog{len:FILTER.len() as u16,filter:FILTER.as_ptr().cast_mut()};
    libc::syscall(libc::SYS_seccomp,1u32,0u32,&raw const bpf)==0
}}
#[allow(unsafe_code)]
fn signals()->bool{unsafe{
    let mut action:libc::sigaction=mem::zeroed();action.sa_sigaction=libc::SIG_DFL;
    let mut empty:libc::sigset_t=mem::zeroed();
    if libc::sigemptyset(&mut action.sa_mask)!=0||libc::sigemptyset(&mut empty)!=0{return false;}
    for s in 1..=64{
        // 32/33 are glibc's reserved threading signals. This is a fresh
        // single-threaded supervisor, not an arbitrary multithreaded host.
        if matches!(s,libc::SIGKILL|libc::SIGSTOP|32|33){continue;}
        if libc::sigaction(s,&action,ptr::null_mut())!=0{return false;}
    }
    libc::sigprocmask(libc::SIG_SETMASK,&empty,ptr::null_mut())==0
}}
#[allow(unsafe_code)]
pub(super) fn exec(id:Option<(u32,u32)>,image:i32,view:i32,parent:i32,ack:i32,null:i32,fds:[i32;3],
    argv:&[*const libc::c_char;3],env:&[*const libc::c_char;7])->!{unsafe{
    // No error text or credential is exported. Keep the original write end
    // CLOEXEC, signal a setup/exec failure, then go directly to _exit.
    let fail=||->!{libc::write(ack,b"!".as_ptr().cast(),1);libc::_exit(126)};
    if !parent_live(parent)||!signals()||libc::getpid()!=1||libc::syscall(libc::SYS_close_range,3u32,u32::MAX,libc::CLOSE_RANGE_CLOEXEC)!=0{fail();}
    let core=libc::rlimit{rlim_cur:0,rlim_max:0};
    if libc::setrlimit(libc::RLIMIT_CORE,&core)!=0||libc::prctl(libc::PR_SET_DUMPABLE,zero(),zero(),zero(),zero())!=0{fail();}
    // The role view is an exact detached tree, not a path rooted in the
    // host. Attach it in this child's private namespace, then enter it.
    if crate::smb_views::enter(view).is_err(){fail();}
    let safe=libc::MS_NODEV|libc::MS_NOSUID|libc::MS_NOEXEC;
    if !mount_child(c"/proc",c"proc",safe|libc::MS_RDONLY,c"hidepid=2,subset=pid"){fail();}
    if id.is_none(){
        for p in [c"/instance/cache",c"/instance/lock",c"/instance/run"]{
            if !mount_child(p,c"tmpfs",safe,c"size=32m,mode=0700,noswap"){fail();}
        }
    }
    // smbd --foreground establishes its own session; only storage
    // detaches here so tty Ctrl+C can never directly signal it.
    if id.is_some()&&libc::setsid()<0{fail();}
    for fd in 0..3{if libc::dup3(null,fd,0)!=fd{fail();}}
    if !install_limits(id)||!parent_live(parent){fail();} // setres[ug]id clears PDEATHSIG
    libc::umask(0o077);
    for (i,src) in fds.iter().enumerate(){if *src>=0&&libc::dup3(*src,(i+3) as i32,0)<0{fail();}}
    libc::syscall(libc::SYS_execveat,image,c"".as_ptr(),argv.as_ptr(),env.as_ptr(),libc::AT_EMPTY_PATH);
    fail()
}}
