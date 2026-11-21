//! Fixed child lifetime primitives for a one-shot CLI, never a generic shell.
//! Only the approved bubblewrap invocation or the separate, fixed helper copy-check worker.
use linux_raw_sys::general as uapi;
use std::{ffi::CString,fs::File,io,mem::MaybeUninit,os::{fd::AsRawFd,unix::{ffi::OsStrExt,process::CommandExt}},process::Command};

use std::sync::atomic::{AtomicBool,Ordering};
static CANCELLED:AtomicBool=AtomicBool::new(false);
extern "C" fn cancel(_:libc::c_int){CANCELLED.store(true,Ordering::Relaxed);}
pub fn cancelled()->bool{CANCELLED.load(Ordering::Relaxed)}

/// Only for the dedicated CLI process. Cancellation is recorded without
/// allocation or non-signal-safe I/O. exec resets caught signals in the child.
pub fn watch_cancellation()->io::Result<()>{
    watch(&[libc::SIGINT,libc::SIGTERM,libc::SIGHUP,libc::SIGQUIT])
}
#[allow(unsafe_code)]
fn watch(signals:&[libc::c_int])->io::Result<()>{
    // SAFETY: fully initialized Linux sigaction with a C-ABI handler that
    // only sets a lock-free x86-64 atomic bool. This does not send a signal.
    unsafe{
        let mut action:libc::sigaction=std::mem::zeroed();
        action.sa_sigaction=cancel as *const () as usize;
        let mut current=MaybeUninit::<libc::sigset_t>::uninit();
        if libc::sigprocmask(libc::SIG_SETMASK,std::ptr::null(),current.as_mut_ptr())!=0{
            return Err(io::Error::last_os_error());
        }
        let current=current.assume_init();
        if libc::sigemptyset(&mut action.sa_mask)!=0{return Err(io::Error::last_os_error());}
        for &sig in signals{
            // A blocked inherited cancellation signal is not observable by our handler.
            if libc::sigismember(&current,sig)!=0{return Err(io::Error::other("diagnostic cancellation signals must be unblocked"));}
            if libc::sigaction(sig,&action,std::ptr::null_mut())!=0{return Err(io::Error::last_os_error());}
        }
    } Ok(())
}

/// Only a dedicated image-check CLI uses this, on its own new readonly
/// description of a previously admitted local regular copy. No replacement
/// lease is taken when a break request has started.
#[allow(unsafe_code)]
pub fn acquire_copy_read_lease(file:&File)->io::Result<()>{
    use rustix::fs::{OFlags,fcntl_getfl};
    let flags=fcntl_getfl(file)?;
    if flags.intersects(OFlags::PATH) || flags & OFlags::ACCMODE != OFlags::RDONLY
        || !file.metadata()?.is_file() || cancelled(){
        return Err(io::Error::other("expected the original readonly diagnostic copy"));
    }
    watch(&[libc::SIGIO])?;
    // Linux uses SIGIO on a newly opened description and initially selects
    // the calling process as recipient. F_SETLEASE is not a waiting lock.
    // SAFETY: fcntl takes an integer lease type and an already open fd.
    if unsafe{libc::fcntl(file.as_raw_fd(),libc::F_SETLEASE,libc::F_RDLCK)}!=0{
        return Err(io::Error::other("cannot lease this copy for reading; it may be in use or leases are unavailable"));
    }
    require_copy_read_lease(file)
}

#[allow(unsafe_code)]
pub fn require_copy_read_lease(file:&File)->io::Result<()>{
    // F_GETLEASE reads this exact open description and reports the target
    // of a pending break as UNLCK, even before any new writer can open it.
    // SAFETY: F_GETLEASE takes no extra argument or buffer.
    let observed=unsafe{libc::fcntl(file.as_raw_fd(),libc::F_GETLEASE)};
    if observed!=libc::F_RDLCK || cancelled(){
        return Err(io::Error::new(io::ErrorKind::Interrupted,"image check cancelled or its read lease was interrupted"));
    }
    Ok(())
}

/// The group leader must stay waitable (therefore its numeric PGID pinned)
/// until the last signal to the dedicated session. Reject auto-reaping.
#[allow(unsafe_code)]
pub fn require_waitable_children()->io::Result<()> {
    let mut a=MaybeUninit::<libc::sigaction>::uninit();
    // SAFETY: no new handler is installed. The kernel initializes a on success.
    if unsafe {libc::sigaction(libc::SIGCHLD,std::ptr::null(),a.as_mut_ptr())} !=0{
        return Err(io::Error::last_os_error());
    }
    // SAFETY: sigaction succeeded above.
    let a=unsafe{a.assume_init()};
    if a.sa_sigaction!=libc::SIG_DFL || a.sa_flags&libc::SA_NOCLDWAIT!=0{
        return Err(io::Error::other("diagnostic monitor needs default, waitable child disposition"));
    }
    Ok(())
}

/// In the spawned child, establish a new session/group *before* any launcher
/// setup, then exec the verified ELF by descriptor. No /proc path or PATH lookup.
/// On any error pre_exec returns Err, never Ok: std cannot execute its nominal path.
pub fn prepare_diagnostic_exec(cmd:&mut Command,original:&File)->io::Result<()>{
    prepare_checker_exec(cmd,original,None)
}

// A full copy hash need not share the checker's CPU allowance. Lower the
// sandbox child's ceiling only, before its landscape or executable changes.
pub fn prepare_leased_diagnostic_exec(cmd:&mut Command,original:&File)->io::Result<()>{
    prepare_checker_exec(cmd,original,Some(300))
}
#[allow(unsafe_code)]
pub fn prepare_xfs_diagnostic_exec(cmd:&mut Command,original:&File)->io::Result<()>{
    // The pinned XFS binary uses sysconf(_SC_NPROCESSORS_ONLN) as a work
    // queue size. With no guest sysfs or procfs, the pinned glibc falls back
    // to this process's affinity. Restrict only this child before exec.
    // SAFETY: stack-only, fixed-length Linux masks; pid 0 is the calling task.
    unsafe{cmd.pre_exec(||{
        let mut available=[0u64;128]; // at most 8192 CPU indices; no fallback
        let bytes=std::mem::size_of_val(&available);
        if libc::sched_getaffinity(0,bytes,available.as_mut_ptr().cast())!=0{
            return Err(io::Error::last_os_error());
        }
        let Some(i)=available.iter().position(|&w|w!=0)else{return Err(io::Error::from_raw_os_error(libc::EINVAL));};
        let mut one=[0u64;128];one[i]=1u64<<available[i].trailing_zeros();
        if libc::sched_setaffinity(0,bytes,one.as_ptr().cast())!=0{
            return Err(io::Error::last_os_error());
        }
        let mut observed=[0u64;128];
        if libc::sched_getaffinity(0,bytes,observed.as_mut_ptr().cast())!=0{
            return Err(io::Error::last_os_error());
        }
        if observed!=one{return Err(io::Error::from_raw_os_error(libc::EINVAL));}
        Ok(())
    });}
    prepare_checker_exec(cmd,original,Some(300))
}
fn prepare_checker_exec(cmd:&mut Command,original:&File,cpu:Option<libc::rlim_t>)->io::Result<()>{
    let mut args=vec![CString::new("bwrap").expect("literal")];
    let ceiling=if cpu.is_some(){79}else{63};
    for arg in cmd.get_args() {
        if args.len()>=ceiling{return Err(io::Error::other("fixed diagnostic launcher argument budget exceeded"));}
        args.push(CString::new(arg.as_bytes()).map_err(|_|io::Error::other("invalid diagnostic launcher argument"))?);
    }
    pinned_exec(cmd,original,args,None,cpu)
}

/// No string from a request can become a subcommand or a program name.
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum CopyFormat{Fat32,Exfat}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum LeasedFormat{Ext4,Xfs,Ntfs,Btrfs}
/// Spawn only the copy checker present in the same protected installation as the
/// calling panel. The only argument is the digest of the *already sealed* stdin.
pub fn prepare_panel_worker_exec(cmd:&mut Command,original:&File,format:CopyFormat,digest:&[u8;32])->io::Result<()>{
    let action=match format{CopyFormat::Fat32=>"__fat32-sealed-v1",CopyFormat::Exfat=>"__exfat-sealed-v1"};
    prepare_fixed_panel_exec(cmd,original,action,digest)
}

/// stdin is the selected saved volume's new readonly
/// description, not a sealed copy. Only this particular worker can lease it.
pub fn prepare_leased_panel_exec(cmd:&mut Command,original:&File,format:LeasedFormat,digest:&[u8;32])->io::Result<()>{
    let action=match format{LeasedFormat::Ext4=>"__ext4-leased-v1",LeasedFormat::Xfs=>"__xfs-leased-v1",LeasedFormat::Ntfs=>"__ntfs-leased-v1",LeasedFormat::Btrfs=>"__btrfs-leased-v1"};
    prepare_fixed_panel_exec(cmd,original,action,digest)
}
fn prepare_fixed_panel_exec(cmd:&mut Command,original:&File,action:&'static str,digest:&[u8;32])->io::Result<()>{
    use std::fmt::Write;
    let mut hex=String::with_capacity(64);for byte in digest{write!(&mut hex,"{byte:02x}").expect("String formatting");}
    let args=vec![CString::new("helper").expect("literal"),CString::new(action).expect("literal"),
        CString::new(hex).expect("hex digest")];
    pinned_exec(cmd,original,args,Some(std::process::id() as libc::pid_t),None)
}

/// Fixed private child kinds. Neither can execute a caller-supplied program
/// or inherit descriptors beyond the same dedicated 3/4 layout.
#[derive(Clone,Copy)]
pub enum OfflineSql { FirstLayout, FirstBrowser, RustAccount, RustVaultWriter, CopyRepairBrowser }

/// The account SQL child retains the operator's TERMINAL session (not
/// the unrelated diagnostic worker session). Only 3=private seqpacket and
/// 4=sealed nonsecret order are introduced; all other caller FDs close.
#[allow(unsafe_code)]
pub fn prepare_account_sql_exec(cmd:&mut Command,original:&File,channel:&File,order:&File,mode:OfflineSql)->io::Result<()>{
    use std::os::fd::AsFd;
    let high=|f:&File|rustix::io::fcntl_dupfd_cloexec(f.as_fd(),16).map(File::from).map_err(io::Error::from);
    let executable=high(original)?;let channel=high(channel)?;let order=high(order)?;
    let parent=std::process::id() as libc::pid_t;
    // SAFETY: Owned, distinct high fds prevent dup3 clobbering the selected ELF.
    // The child calls only async-safe syscalls on fixed memory before exec,
    // never enables shell fallback, reallocates or uses a caller-supplied arg.
    unsafe{cmd.pre_exec(move||{
        let mut action:libc::sigaction=std::mem::zeroed();action.sa_sigaction=libc::SIG_DFL;
        let mut term:libc::sigset_t=std::mem::zeroed();
        if libc::sigemptyset(&mut action.sa_mask)!=0||libc::sigemptyset(&mut term)!=0
            ||libc::sigaddset(&mut term,libc::SIGTERM)!=0
            ||libc::sigaction(libc::SIGTERM,&action,std::ptr::null_mut())!=0
            ||libc::sigprocmask(libc::SIG_UNBLOCK,&term,std::ptr::null_mut())!=0
            ||libc::syscall(libc::SYS_close_range,3u32,u32::MAX,libc::CLOSE_RANGE_CLOEXEC)!=0
            ||libc::prctl(libc::PR_SET_PDEATHSIG,libc::SIGTERM as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong)!=0{
            return Err(io::Error::last_os_error());
        }
        if libc::getppid()!=parent{return Err(io::Error::from_raw_os_error(libc::ECANCELED));}
        if libc::dup3(channel.as_raw_fd(),3,0)!=3||libc::dup3(order.as_raw_fd(),4,0)!=4{
            return Err(io::Error::last_os_error());
        }
        let action=match mode{OfflineSql::FirstLayout=>c"__first-layout-sql-v9",OfflineSql::FirstBrowser=>c"__first-install-browser-v1",
            OfflineSql::RustAccount=>c"__zxs-shared-account-sql-v2",OfflineSql::RustVaultWriter=>c"__zxs-shared-vault-writer-v2",
            OfflineSql::CopyRepairBrowser=>c"__copy-repair-browser-v2",
        };
        let argv=[c"helper".as_ptr(),action.as_ptr(),std::ptr::null()];
        let env=[c"LC_ALL=C".as_ptr(),std::ptr::null()];
        libc::syscall(uapi::__NR_execveat as libc::c_long,executable.as_raw_fd(),c"".as_ptr(),
            argv.as_ptr(),env.as_ptr(),libc::AT_EMPTY_PATH);
        Err(io::Error::last_os_error())
    });}
    Ok(())
}

/// Only the fixed exec-child entry uses this, after proving that exactly 3/4
/// already exist. Mark both CLOEXEC before any future system action.
#[allow(unsafe_code)]
pub fn account_sql_descriptors()->io::Result<[File;2]>{
    use std::os::fd::FromRawFd;
    if unsafe{libc::fcntl(3,libc::F_SETFD,libc::FD_CLOEXEC)}<0
        ||unsafe{libc::fcntl(4,libc::F_SETFD,libc::FD_CLOEXEC)}<0{return Err(io::Error::last_os_error());}
    // SAFETY: application-specific startup inherits 3 and 4 by ownership,
    // both accepted by the kernel above, and no other owner was constructed.
    Ok(unsafe{[File::from_raw_fd(3),File::from_raw_fd(4)]})
}
#[allow(unsafe_code)]
fn pinned_exec(cmd:&mut Command,original:&File,args:Vec<CString>,panel:Option<libc::pid_t>,cpu:Option<libc::rlim_t>)->io::Result<()> {
    let executable=original.try_clone()?;
    // SAFETY: all memory and C strings are owned and allocated before fork.
    // The closure only builds pointer arrays on the stack and uses async-safe
    // Linux syscalls. No lock, allocation or unwinding. The copy worker
    // alone inherits its selected operand as stdin + stdout/stderr pipes;
    // never the listener.
    unsafe {cmd.pre_exec(move||{
        if let Some(parent)=panel {
            // SIG_IGN and the signal mask can survive exec. Make termination
            // effective *before* arming PDEATHSIG; a lost parent before arming
            // is detected by the getppid check. No checker exists in this window.
            let mut action:libc::sigaction=std::mem::zeroed();action.sa_sigaction=libc::SIG_DFL;
            let mut term:libc::sigset_t=std::mem::zeroed();
            if libc::sigemptyset(&mut action.sa_mask)!=0 || libc::sigemptyset(&mut term)!=0
                || libc::sigaddset(&mut term,libc::SIGTERM)!=0
                || libc::sigaction(libc::SIGTERM,&action,std::ptr::null_mut())!=0
                || libc::sigprocmask(libc::SIG_UNBLOCK,&term,std::ptr::null_mut())!=0 {
                return Err(io::Error::last_os_error());
            }
            if libc::syscall(libc::SYS_close_range,3u32,u32::MAX,libc::CLOSE_RANGE_CLOEXEC)!=0
                || libc::prctl(libc::PR_SET_PDEATHSIG,libc::SIGTERM as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong)!=0 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid()!=parent{return Err(io::Error::from_raw_os_error(libc::ECANCELED));}
        }
        if libc::setsid()<0{return Err(io::Error::last_os_error());}
        if let Some(limit)=cpu{
            let mut value=MaybeUninit::<libc::rlimit>::uninit();
            if libc::getrlimit(libc::RLIMIT_CPU,value.as_mut_ptr())!=0{return Err(io::Error::last_os_error());}
            let mut value=value.assume_init();
            value.rlim_max=value.rlim_max.min(limit);
            value.rlim_cur=value.rlim_cur.min(value.rlim_max);
            if libc::setrlimit(libc::RLIMIT_CPU,&value)!=0{return Err(io::Error::last_os_error());}
        }
        let mut argv=[std::ptr::null::<libc::c_char>();80];
        for (dst,src) in argv.iter_mut().zip(args.iter()){*dst=src.as_ptr();}
        let env=[c"LC_ALL=C".as_ptr(),std::ptr::null()];
        libc::syscall(uapi::__NR_execveat as libc::c_long,executable.as_raw_fd(),c"".as_ptr(),
                      argv.as_ptr(),env.as_ptr(),libc::AT_EMPTY_PATH);
        Err(io::Error::last_os_error())
    });}
    Ok(())
}

/// Native x86-64 classic BPF for --seccomp. Only group-lifetime constraints,
/// not a general syscall security policy. Never allow independent children.
pub fn diagnostic_group_filter()->Vec<u8>{group_filter(false,false)}

// The pinned libext2fs checks bitmaps in threads on a sufficiently large
// filesystem. Allow only the actual glibc 2.41 NPTL thread in this profile.
pub fn ext4_group_filter()->Vec<u8>{group_filter(true,false)}
// The pinned XFS xfile path falls back to O_TMPFILE in its bounded private
// tmpfs. Do not let it use unlimited anonymous shmem instead.
pub fn xfs_group_filter()->Vec<u8>{group_filter(true,true)}
const NPTL_CLONE:u32=uapi::CLONE_VM|uapi::CLONE_FS|uapi::CLONE_FILES|uapi::CLONE_SIGHAND
    |uapi::CLONE_THREAD|uapi::CLONE_SYSVSEM|uapi::CLONE_SETTLS|uapi::CLONE_PARENT_SETTID
    |uapi::CLONE_CHILD_CLEARTID;
fn group_filter(threads:bool,xfs:bool)->Vec<u8>{
    const LD_W_ABS:u16=0x20;const JEQ:u16=0x15;const JGE:u16=0x35;const RET:u16=0x06;
    const KILL:u32=0x8000_0000;const EPERM:u32=0x0005_0001;const ALLOW:u32=0x7fff_0000;
    let mut out=Vec::with_capacity(192);
    let mut op=|code:u16,jt:u8,jf:u8,k:u32|{
        out.extend_from_slice(&code.to_ne_bytes());out.extend_from_slice(&[jt,jf]);out.extend_from_slice(&k.to_ne_bytes());
    };
    op(LD_W_ABS,0,0,4);op(JEQ,1,0,0xc000_003e);op(RET,0,0,KILL); // seccomp_data.arch
    op(LD_W_ABS,0,0,0);op(JGE,0,1,0x4000_0000);op(RET,0,0,KILL); // seccomp_data.nr, reject x32
    for number in [uapi::__NR_setsid,uapi::__NR_setpgid,uapi::__NR_fork,uapi::__NR_vfork,
                   uapi::__NR_unshare,uapi::__NR_setns] {
        op(JEQ,0,1,number);op(RET,0,0,EPERM);
    }
    if xfs{
        for number in [uapi::__NR_memfd_create,uapi::__NR_sched_setaffinity]{
            op(JEQ,0,1,number);op(RET,0,0,EPERM);
        }
    }
    if threads{
        // Do not dereference clone3's hidden argument struct. Its defined
        // ENOSYS path in glibc uses the inspectable x86-64 clone syscall.
        op(JEQ,0,1,uapi::__NR_clone3);op(RET,0,0,0x0005_0000|libc::ENOSYS as u32);
        op(JEQ,0,6,uapi::__NR_clone);
        op(LD_W_ABS,0,0,20);op(JEQ,1,0,0);op(RET,0,0,EPERM); // flags high
        op(LD_W_ABS,0,0,16);op(JEQ,1,0,NPTL_CLONE);op(RET,0,0,EPERM); // low, including zero exit signal
    }else{
        for number in [uapi::__NR_clone,uapi::__NR_clone3]{
            op(JEQ,0,1,number);op(RET,0,0,EPERM);
        }
    }
    op(RET,0,0,ALLOW);out
}

#[cfg(test)]
mod tests {
    use super::*;
    // Independent interpreter restricted to the four emitted classic-BPF operations.
    fn decision_with_flags(p:&[u8],arch:u32,nr:u32,flags:u64)->u32{
        let (mut a,mut pc)=(0,0usize);
        loop {
            let f=&p[pc*8..(pc+1)*8];pc+=1;
            let ins=u16::from_ne_bytes(f[..2].try_into().unwrap());
            let k=u32::from_ne_bytes(f[4..].try_into().unwrap());
            match ins{
                0x20=>a=match k{0=>nr,4=>arch,16=>flags as u32,20=>(flags>>32) as u32,_=>panic!("unexpected seccomp_data offset")},
                0x15=>pc+=usize::from(if a==k{f[2]}else{f[3]}),
                0x35=>pc+=usize::from(if a>=k{f[2]}else{f[3]}),
                0x06=>return k,_=>panic!("unexpected fixed diagnostic instruction"),
            }
        }
    }
    fn decision(p:&[u8],arch:u32,nr:u32)->u32{decision_with_flags(p,arch,nr,0)}
    #[test]
    fn leased_checkers_can_create_an_exact_glibc_same_process_thread(){
        let arch=0xc000_003e;
        let glibc_flags=0x003d_0f00;
        let ext=ext4_group_filter();
        let raw=|flags|decision_with_flags(&ext,arch,uapi::__NR_clone,flags);
        assert_eq!(raw(glibc_flags),0x7fff_0000);
        for flags in [0,17,glibc_flags|17,glibc_flags&!u64::from(uapi::CLONE_THREAD),
                      glibc_flags|u64::from(uapi::CLONE_NEWUSER),glibc_flags|(1<<32)]{
            assert_eq!(raw(flags),0x0005_0001);
        }
        assert_eq!(decision(&ext,arch,uapi::__NR_clone3),0x0005_0026);
        for nr in [uapi::__NR_fork,uapi::__NR_vfork,uapi::__NR_setpgid,uapi::__NR_setsid,uapi::__NR_unshare,uapi::__NR_setns]{
            assert_eq!(decision(&ext,arch,nr),0x0005_0001);
        }
        assert_eq!(decision(&ext,arch,uapi::__NR_fstat),0x7fff_0000);
        assert_eq!(decision_with_flags(&diagnostic_group_filter(),arch,uapi::__NR_clone,glibc_flags),0x0005_0001);
    }
    #[test]
    fn xfs_requires_bounded_scratch_and_keeps_its_own_verified_launch_affinity(){
        let arch=0xc000_003e;
        let p=xfs_group_filter();
        assert_eq!(decision_with_flags(&p,arch,uapi::__NR_clone,0x003d_0f00),0x7fff_0000);
        assert_eq!(decision_with_flags(&p,arch,uapi::__NR_clone,0x003d_0f11),0x0005_0001);
        for n in [uapi::__NR_memfd_create,uapi::__NR_sched_setaffinity]{
            assert_eq!(decision(&p,arch,n),0x0005_0001);
            assert_eq!(decision(&ext4_group_filter(),arch,n),0x7fff_0000);
        }
        assert_eq!(decision(&p,arch,uapi::__NR_sched_getaffinity),0x7fff_0000);
        assert_eq!(decision(&p,arch,uapi::__NR_openat),0x7fff_0000);
        assert_eq!(decision(&p,arch,uapi::__NR_setns),0x0005_0001);
    }
    #[test]
    fn tool_cannot_fork_or_leave_the_launchers_process_group_in_another_abi(){
        let p=diagnostic_group_filter();let arch=0xc000_003e;
        for n in [uapi::__NR_fork,uapi::__NR_vfork,uapi::__NR_clone,uapi::__NR_clone3,uapi::__NR_setpgid,uapi::__NR_setsid,uapi::__NR_unshare,uapi::__NR_setns]{
            assert_eq!(decision(&p,arch,n),0x0005_0001);
        }
        for n in [uapi::__NR_read,uapi::__NR_execve,uapi::__NR_exit_group]{assert_eq!(decision(&p,arch,n),0x7fff_0000);}
        assert_eq!(decision(&p,0x4000_0003,uapi::__NR_fork),0x8000_0000);
        assert_eq!(decision(&p,arch,0x4000_0000|uapi::__NR_clone),0x8000_0000);
    }
}

/// Only this kernel null object may be exposed as a device. Its pathname is
/// never taken from the environment, manifest or command arguments.
pub fn null_device()->io::Result<File>{
    use rustix::fs::{Mode,OFlags,PROC_SUPER_MAGIC,fstatfs,open,openat};
    use std::os::unix::fs::{FileTypeExt,MetadataExt};
    const DIR:OFlags=OFlags::PATH.union(OFlags::DIRECTORY).union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);
    let fail=||io::Error::other("the original kernel null device could not be proved");
    let dev=File::from(open(c"/dev",DIR,Mode::empty())?);
    let pin=File::from(openat(&dev,c"null",OFlags::PATH|OFlags::CLOEXEC|OFlags::NOFOLLOW,Mode::empty())?);
    let m=pin.metadata()?;
    if !m.file_type().is_char_device()||m.rdev()!=rustix::fs::makedev(1,3)||m.uid()!=0||m.gid()!=0||
        m.mode()&0o7777!=0o666||!matches!(fstatfs(&pin)?.f_type as u64,0x01021994){return Err(fail());}
    // Use the genuine, held current proc-fd identity, not a second /dev name.
    let p=File::from(open(c"/proc",DIR,Mode::empty())?);
    if fstatfs(&p)?.f_type!=PROC_SUPER_MAGIC{return Err(fail());}
    let name=format!("thread-self/fd/{}",pin.as_raw_fd());
    let file=File::from(openat(&p,name.as_str(),OFlags::RDWR|OFlags::NOCTTY|OFlags::CLOEXEC,Mode::empty())?);
    let f=file.metadata()?;
    if !f.file_type().is_char_device()||(f.dev(),f.ino(),f.rdev())!=(m.dev(),m.ino(),m.rdev()){return Err(fail());}
    Ok(file)
}
