//! Own only a child born in this exact clone3 call. A PID string merely
//! routes to the child's proc entry after pidfd waitability; it is never
//! accepted as a signal, wait, ready or ownership capability.
mod child;
use super::{Fault,ImageStamp,Namespace,Parent,PreparedHandoff,PROC,Result,bounded,caps,current_task,
    follow_kernel_link,foreground,kernel,object,proc_dir,row,view,words};
use kernel::{BootSocket,OriginalProcess};
use crate::smb::launch::transport::{child_receipt,offer_packet};
use std::{ffi::{CStr,CString},fs::File,io::{self,Read},mem::{size_of,zeroed},
    os::{fd::{AsRawFd,BorrowedFd,RawFd},unix::fs::MetadataExt},thread,time::{Duration,Instant}};

#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) enum Role{Preflight,Server,Web}
impl Role{
    fn marker(self)->&'static CStr{
        match self{
            Self::Preflight=>c"zxs:internal-smb-preflight-311",
            Self::Server=>c"zxs:internal-smb-311",
            Self::Web=>c"zxs:internal-web-1",
        }
    }
}
#[derive(Clone,Copy,Eq,PartialEq)]
enum Phase{Unconfirmed,Exposed,Ready,TermStarted,AbortStarted}

#[must_use]
pub(super) struct Born<'a>{
    parent:&'a Parent,original:File,ack:File,done:Option<i32>,
    channel:Option<BootSocket>,phase:Phase,
    image:File,view:File,uid:u32,gid:u32,role:Role,
}

impl<'parent> PreparedHandoff<'parent>{
    /// Caller must still hold actual independent site, approval, plan and
    /// data-source proofs. Kernel lifetime alone is never storage authority.
    pub(super) fn birth(self,approved:&File,view:&view::View<'_>,
        subject:(u32,u32),role:Role)->Result<Born<'parent>>{
        let (uid,gid)=subject;
        if view.role()!=role||[uid,gid].iter().any(|n|matches!(*n,0|65534|u32::MAX)){return Err(Fault::Handoff);}
        self.parent.same_image(approved)?;view.require_prepared(self.parent)?;
        let null=high(null_device(self.parent)?.as_raw_fd())?;
        let original_image=approved.try_clone().map_err(|_|Fault::Handoff)?;
        let original_view=view.file().try_clone().map_err(|_|Fault::Handoff)?;
        let image=high(original_image.as_raw_fd())?;
        let mount=high(original_view.as_raw_fd())?;
        let parent=high(self.parent.original.as_raw_fd())?;
        let peer=high(self.child.as_raw_fd())?;
        let (ack,error)=pipe()?;
        let fail=high(error.as_raw_fd())?;drop(error);
        self.parent.require()?;
        let context=child::Inputs{image:image.as_raw_fd(),view:mount.as_raw_fd(),parent:parent.as_raw_fd(),
            peer:peer.as_raw_fd(),fail:fail.as_raw_fd(),null:null.as_raw_fd(),role,uid,gid};
        let mut pidfd=-1i32;
        // Linux clone_args is eleven ordered u64 fields. All unused fields
        // are zero. No VM, fd table, fs_struct, signal handlers or vfork shared.
        let mut args=[0u64;11];
        args[0]=(libc::CLONE_PIDFD|libc::CLONE_NEWPID|libc::CLONE_NEWNET|libc::CLONE_NEWIPC|libc::CLONE_NEWUTS)as u64;
        args[1]=(&raw mut pidfd)as usize as u64;
        args[4]=libc::SIGCHLD as u64;
        // SAFETY: fixed Linux 6.12 ABI, valid pidfd output; the child branch
        // immediately enters the static allocation-free setup below.
        let rc=unsafe{libc::syscall(libc::SYS_clone3,args.as_ptr(),size_of::<[u64;11]>())};
        if rc<0{return Err(Fault::Handoff);}
        if rc==0{unsafe{child::start(context)}}
        // SAFETY: success with CLONE_PIDFD atomically transfers one fresh
        // pidfd. Install custody before any operation that could fail.
        let original=unsafe{File::from(kernel::new_fd(i64::from(pidfd))?)};
        let born=Born{parent:self.parent,original,ack,done:None,channel:Some(self.host),phase:Phase::Unconfirmed,
            image:original_image,view:original_view,uid,gid,role};
        drop(fail);drop(peer);
        drop(self.child); // parent may not hold a backup of the child's endpoint
        Ok(born)
    }
}
impl Born<'_>{
    pub(super) fn handoff_identity(&self)->Result<OriginalProcess>{
        OriginalProcess::pin(self.original.try_clone().map_err(|_|Fault::Handoff)?)
    }
    pub(super) fn subject(&self)->(u32,u32){(self.uid,self.gid)}
    pub(super) fn await_exec(&mut self,until:Instant)->Result<()>{
        let mut byte=[0];
        loop{
            if Instant::now()>=until||self.peek()?.is_some(){return Err(Fault::Handoff);}
            match self.ack.read(&mut byte){
                Ok(0)=>break, // parent must still verify exact live image and namespace
                Ok(_)=>return Err(Fault::Handoff),
                Err(e) if matches!(e.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::Interrupted)=>(),
                Err(_)=>return Err(Fault::Handoff),
            }
            thread::sleep(Duration::from_millis(5));
        }
        self.snapshot().map(|_|())
    }
    pub(super) fn peek(&self)->Result<Option<i32>>{
        if let Some(n)=self.done{return Ok(Some(n));}
        wait(&self.original,libc::WNOHANG|libc::WNOWAIT)
    }
    pub(super) fn reap(&mut self)->Result<i32>{
        if let Some(n)=self.done{return Ok(n);}
        let status=wait(&self.original,0)?.ok_or(Fault::Handoff)?;
        self.done=Some(status);
        Ok(status)
    }
    pub(super) fn snapshot(&self)->Result<bool>{
        if self.peek()?.is_some(){return Err(Fault::Handoff);}
        self.parent.same_image(&self.image)?;
        let task=task(self.parent,&self.original)?;
        let id=task.0;
        let scope=task.1;
        let image=follow_kernel_link(&scope,c"exe",libc::O_RDONLY|libc::O_NONBLOCK|libc::O_NOCTTY)?;
        let root=follow_kernel_link(&scope,c"root",libc::O_PATH|libc::O_DIRECTORY)?;
        if ImageStamp::read(&image)?!=ImageStamp::read(&self.image)?||object(&root)?!=object(&self.view)?{
            return Err(Fault::Handoff);
        }
        let proc=view_proc(&root)?;
        if kernel::fs_magic(proc.as_raw_fd())?!=PROC||proc.metadata().map_err(|_|Fault::Handoff)?.ino()!=1||
            view_flags(&proc)?&(libc::ST_RDONLY|libc::ST_NODEV|libc::ST_NOSUID|libc::ST_NOEXEC)
                !=libc::ST_RDONLY|libc::ST_NODEV|libc::ST_NOSUID|libc::ST_NOEXEC{return Err(Fault::Handoff);}
        let low=self.handoff_identity()?;
        let parent=&self.parent.original;
        let own_pid=Namespace::Pid.of_parent(&low)?;
        let owner=Namespace::User.owner_of_pid(&own_pid)?;
        if object(&own_pid)?==object(&Namespace::Pid.of_parent(parent)?)?||
            object(&own_pid)?!=object(&Namespace::PidForChildren.of_parent(&low)?)?||
            object(&Namespace::User.of_parent(&low)?)?!=object(&Namespace::User.of_parent(parent)?)?||
            object(&owner)?!=object(&Namespace::User.of_parent(parent)?)?{
            return Err(Fault::Handoff);
        }
        for n in [Namespace::Mount,Namespace::Net,Namespace::Ipc,Namespace::Uts]{
            if object(&n.of_parent(&low)?)?==object(&n.of_parent(parent)?)?{return Err(Fault::Handoff);}
        }
        let fields=bounded(&scope,c"status",65_536)?;
        let p=std::process::id().to_string();
        let u=self.uid.to_string();let g=self.gid.to_string();
        for key in [b"Pid:".as_slice(),b"Tgid:"]{
            if !words(row(&fields,key)?).eq([id.as_bytes()]){return Err(Fault::Handoff);}
        }
        for key in [b"NSpid:".as_slice(),b"NSpgid:",b"NSsid:"]{
            if !words(row(&fields,key)?).eq([id.as_bytes(),b"1"]){return Err(Fault::Handoff);}
        }
        if !words(row(&fields,b"PPid:")?).eq([p.as_bytes()])||!words(row(&fields,b"Uid:")?).eq([u.as_bytes();4])||
            !words(row(&fields,b"Gid:")?).eq([g.as_bytes();4])||words(row(&fields,b"Groups:")?).next().is_some()||
            !words(row(&fields,b"NoNewPrivs:")?).eq([b"1".as_slice()])||!words(row(&fields,b"Seccomp:")?).eq([b"2".as_slice()]){
            return Err(Fault::Handoff);
        }
        for key in [b"CapInh:".as_slice(),b"CapPrm:",b"CapEff:",b"CapBnd:",b"CapAmb:"]{
            if caps(&fields,key)?!=0{return Err(Fault::Handoff);}
        }
        let term=1u64<<(libc::SIGTERM-1);
        let graceful=caps(&fields,b"SigCgt:")?&term!=0&&caps(&fields,b"SigBlk:")?&term==0;
        if ImageStamp::read(&follow_kernel_link(&scope,c"exe",libc::O_RDONLY|libc::O_NONBLOCK|libc::O_NOCTTY)?)?!=ImageStamp::read(&self.image)?||
            object(&follow_kernel_link(&scope,c"root",libc::O_PATH|libc::O_DIRECTORY)?)?!=object(&self.view)?||
            self.peek()?.is_some(){return Err(Fault::Handoff);}
        self.parent.require()?;Ok(graceful)
    }
    // Actual pre-START custody proved this own pidfd, reduced image and
    // live TERM handler. Once serving rights may have left, even a lost
    // READY is TERM-only. No later path proof or numeric PID is needed.
    fn begin_drain(&mut self)->Result<()>{
        match self.phase{
            Phase::TermStarted=>return Ok(()),
            Phase::Ready|Phase::Exposed=>{},
            _=>return Err(Fault::Handoff),
        }
        self.initiated(libc::SIGTERM)?;
        self.phase=Phase::TermStarted;
        Ok(())
    }
    pub(super) fn begin_failed_launch(&mut self)->Result<()>{
        match self.phase{
            Phase::Unconfirmed|Phase::AbortStarted=>self.begin_abort(),
            _=>self.begin_drain(),
        }
    }
    fn wait_term_handlers(&self,until:Instant)->Result<()>{
        loop{
            foreground::require()?;
            if Instant::now()>=until{return Err(Fault::Handoff);}
            if self.snapshot()?{return Ok(());}
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn begin_abort(&mut self)->Result<()>{
        match self.phase{
            Phase::AbortStarted=>return Ok(()),
            Phase::Unconfirmed=>{},
            _=>return Err(Fault::Handoff),
        }
        self.initiated(libc::SIGKILL)?;
        self.phase=Phase::AbortStarted;
        Ok(())
    }
    fn initiated(&self,sig:libc::c_int)->Result<()>{
        match self.signal(sig){
            Ok(())=>Ok(()),
            Err(e) if e.raw_os_error()==Some(libc::ESRCH)&&self.peek()?.is_some()=>Ok(()),
            Err(_)=>Err(Fault::Handoff),
        }
    }
    fn finish_drain(&mut self)->Result<()>{
        if self.phase!=Phase::TermStarted{return Err(Fault::Handoff);}
        // Only serving rights possibly offered or exact READY here. No elapsed time or
        // observation error can upgrade a real drain to a forced kill.
        loop{
            if let Some(seen)=self.peek()?{
                let reaped=self.reap()?;
                return if seen==0&&reaped==0{Ok(())}else{Err(Fault::Handoff)};
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn signal(&self,sig:libc::c_int)->io::Result<()>{
        if self.done.is_some(){return Err(io::Error::other("child already reaped"));}
        // SAFETY: only a fixed TERM or KILL on the pidfd born by this object.
        let code=unsafe{libc::syscall(libc::SYS_pidfd_send_signal,self.original.as_raw_fd(),
            sig, std::ptr::null::<libc::siginfo_t>(),0u32)};
        if code==0{Ok(())}else{Err(io::Error::last_os_error())}
    }
}
/// An exact PREPARED token requires the original packet and a later live
/// image/ROOT/TERM snapshot. The TERM-only obligation already held at the
/// possibly delivered START vote and never depends on this token.
#[must_use]
pub(super) struct Ready<'a>(Born<'a>);
impl<'a> Born<'a>{
    fn admitted(mut self)->Ready<'a>{
        self.phase=Phase::Ready;
        Ready(self)
    }
    pub(super) fn preflight(mut self,frame:&[u8;80],public:&File,
        until:Instant)->Result<([u8;80],File)>{
        if self.role!=Role::Preflight||&frame[..16]!=b"ZXS-RUST/PREFL1\0"{return Err(Fault::Handoff);}
        self.await_exec(until)?;
        let original=self.handoff_identity()?;
        let channel=self.channel.take().ok_or(Fault::Handoff)?;
        foreground::require()?;
        offer_packet(&channel,frame,public,&[],until,foreground::require)?;
        let (reply,body)=child_receipt(&channel,&original,self.subject(),true,until)?;
        if &reply[..16]!=b"ZXS-RUST/PFDONE\0"||reply[16..48]!=frame[16..48]{return Err(Fault::Handoff);}
        loop{
            if Instant::now()>=until{return Err(Fault::Handoff);}
            if let Some(seen)=self.peek()?{
                let reaped=self.reap()?;
                if seen!=0||reaped!=0{return Err(Fault::Handoff);}
                return Ok((reply,body.ok_or(Fault::Handoff)?));
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    pub(super) fn server(mut self,frame:&[u8;80],public:&File,
        vault_and_listeners:&[BorrowedFd<'_>],until:Instant,before_send:impl FnOnce()->Result<()>)->Result<Ready<'a>>{
        if self.role!=Role::Server||&frame[..16]!=b"ZXS-RUST/START2\0"||
            !(3..=18).contains(&vault_and_listeners.len()){return Err(Fault::Handoff);}
        let mut wanted=*frame;wanted[..16].copy_from_slice(b"ZXS-RUST/PREP02\0");
        self.await_exec(until)?;
        let original=self.handoff_identity()?;
        let channel=self.channel.take().ok_or(Fault::Handoff)?;
        self.wait_term_handlers(until)?;
        before_send()?; // after verifying the actual exec; no rights on a revoked scope
        offer_packet(&channel,frame,public,vault_and_listeners,until,||self.expose())?;
        let (reply,_)=child_receipt(&channel,&original,self.subject(),false,until)?;
        if reply!=wanted||!self.snapshot()?{return Err(Fault::Handoff);}
        Ok(self.admitted())
    }
    /// On failure the one owning Web birth comes back to the same
    /// foreground. It may not reap before starting the already-PREPARED
    /// sibling's drain, even when the Web identity is still uncertain.
    pub(super) fn web(mut self,frame:&[u8;80],public:&File,listener:BorrowedFd<'_>,gate:BorrowedFd<'_>,
        until:Instant,before_send:impl FnOnce()->Result<()>)->std::result::Result<Ready<'a>,(Fault,Self)>{
        let verified=(||{
            if self.role!=Role::Web||&frame[..16]!=crate::smb::launch::web::START{return Err(Fault::Handoff);}
            let mut wanted=*frame;wanted[..16].copy_from_slice(crate::smb::launch::web::PREPARED);
            self.await_exec(until)?;
            let original=self.handoff_identity()?;
            let channel=self.channel.take().ok_or(Fault::Handoff)?;
            self.wait_term_handlers(until)?;
            before_send()?;
            offer_packet(&channel,frame,public,&[listener,gate],until,||self.expose())?;
            let (got,_)=child_receipt(&channel,&original,self.subject(),false,until)?;
            if got!=wanted||!self.snapshot()?{return Err(Fault::Handoff);}
            Ok(())
        })();
        match verified{Ok(())=>Ok(self.admitted()),Err(e)=>Err((e,self))}
    }
    fn expose(&mut self)->Result<()>{
        foreground::require()?;
        self.phase=Phase::Exposed; // unknown send is never treated as pre-service
        Ok(())
    }
}
impl Ready<'_>{
    pub(super) fn is_smb(&self)->bool{self.0.role==Role::Server}
    pub(super) fn is_web(&self)->bool{self.0.role==Role::Web}
    pub(super) fn peek(&self)->Result<Option<i32>>{self.0.peek()}
    pub(super) fn begin_drain(&mut self)->Result<()>{self.0.begin_drain()}
    pub(super) fn finish(mut self)->Result<()>{self.0.finish_drain()}
    pub(super) fn stop(mut self)->Result<()>{self.begin_drain()?;self.finish()}
}
impl Drop for Born<'_>{
    fn drop(&mut self){
        if self.done.is_some(){return;}
        drop(self.channel.take());
        // Do not return to another ZXS component or release the caller's
        // real leases while reaping is unproven. A rejected pidfd signal or
        // wait is not an exit receipt. Keep the same descriptor and retry
        // at a bounded rate; do not adopt a numeric PID or another wait.
        // If TERM was already delivered, even an observation error must
        // never turn the normal drain into a forced kill.
        loop{
            match self.phase{
                Phase::Ready|Phase::Exposed=>{let _=self.begin_drain();}
                Phase::Unconfirmed=>{let _=self.begin_abort();}
                Phase::TermStarted|Phase::AbortStarted=>if self.reap().is_ok(){return;},
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}
fn wait(pidfd:&File,options:libc::c_int)->Result<Option<i32>>{
    loop{
        // SAFETY: fixed pidfd and sized initialized C output, never a
        // numeric PID; only EXITED states are requested.
        let mut s:libc::siginfo_t=unsafe{zeroed()};
        if unsafe{libc::waitid(libc::P_PIDFD,pidfd.as_raw_fd() as libc::id_t,&raw mut s,libc::WEXITED|options)}!=0{
            if io::Error::last_os_error().kind()==io::ErrorKind::Interrupted{continue;}
            return Err(Fault::Handoff);
        }
        // SAFETY: waitid initialized the child information, or zero for no state.
        let pid=unsafe{s.si_pid()};
        if pid==0{return Ok(None);}
        if pid<2||s.si_signo!=libc::SIGCHLD{return Err(Fault::Handoff);}
        let value=match s.si_code{
            libc::CLD_EXITED=>unsafe{s.si_status()},
            libc::CLD_KILLED|libc::CLD_DUMPED=>-1,
            _=>return Err(Fault::Handoff),
        };
        return Ok(Some(value));
    }
}
fn high(fd:RawFd)->Result<File>{
    // SAFETY: F_DUPFD_CLOEXEC produces a sole new descriptor >= 16.
    unsafe{Ok(File::from(kernel::new_fd(i64::from(libc::fcntl(fd,libc::F_DUPFD_CLOEXEC,16)))?))}
}
fn pipe()->Result<(File,File)>{
    let mut io=[-1;2];
    // SAFETY: two live output slots, anonymous nonblocking error pipe.
    if unsafe{libc::pipe2(io.as_mut_ptr(),libc::O_CLOEXEC|libc::O_NONBLOCK)}!=0{return Err(Fault::Handoff);}
    // SAFETY: these are two fresh distinct descriptors.
    unsafe{Ok((File::from(kernel::new_fd(i64::from(io[0]))?),File::from(kernel::new_fd(i64::from(io[1]))?)))}
}
fn null_device(parent:&Parent)->Result<File>{
    use std::os::unix::fs::FileTypeExt;
    let root=parent.host()?;
    // SAFETY: only the fixed host device directory and its null child;
    // metadata is checked before opening the actual pinned character inode.
    unsafe{
        let d=File::from(kernel::new_fd(i64::from(libc::openat(root.as_raw_fd(),c"dev".as_ptr(),
            libc::O_PATH|libc::O_NOFOLLOW|libc::O_CLOEXEC)))?);
        if !d.metadata().map_err(|_|Fault::Handoff)?.is_dir(){return Err(Fault::Handoff);}
        let pin=File::from(kernel::new_fd(i64::from(libc::openat(d.as_raw_fd(),c"null".as_ptr(),
            libc::O_PATH|libc::O_NOFOLLOW|libc::O_CLOEXEC)))?);
        let m=pin.metadata().map_err(|_|Fault::Handoff)?;
        if !m.file_type().is_char_device()||m.mode()&0o7777!=0o666||m.uid()!=0||m.gid()!=0||
            (libc::major(m.rdev()),libc::minor(m.rdev()))!=(1,3)||
            kernel::fs_magic(pin.as_raw_fd())?!=0x0102_1994{return Err(Fault::Handoff);}
        let task=current_task(&parent.proc)?;
        let own=proc_dir(&task,c"fd")?;
        let decimal=CString::new(pin.as_raw_fd().to_string()).map_err(|_|Fault::Handoff)?;
        let null=follow_kernel_link(&own,&decimal,libc::O_RDWR|libc::O_NOCTTY)?;
        if object(&null)?!=object(&pin)?{return Err(Fault::Handoff);}
        Ok(null)
    }
}
fn task(parent:&Parent,pidfd:&File)->Result<(String,File)>{
    let own=current_task(&parent.proc)?;
    let info=proc_dir(&own,c"fdinfo")?;
    let descriptor=CString::new(pidfd.as_raw_fd().to_string()).map_err(|_|Fault::Handoff)?;
    let b=bounded(&info,&descriptor,4096)?;
    let mut fields=words(row(&b,b"Pid:")?);
    let raw=fields.next().ok_or(Fault::Handoff)?;
    if fields.next().is_some()||raw.is_empty()||raw.len()>10||raw[0]==b'0'||!raw.iter().all(u8::is_ascii_digit)||
        !words(row(&b,b"NSpid:")?).eq([raw,b"1"]){return Err(Fault::Handoff);}
    let id=std::str::from_utf8(raw).map_err(|_|Fault::Handoff)?.to_owned();
    let name=CString::new(raw).map_err(|_|Fault::Handoff)?;
    Ok((id,proc_dir(&parent.proc,&name)?))
}
fn view_proc(root:&File)->Result<File>{
    // SAFETY: resolve only the mounted proc leaf of the actual child root.
    let mut h:libc::open_how=unsafe{zeroed()};
    h.flags=(libc::O_PATH|libc::O_NOFOLLOW|libc::O_CLOEXEC)as u64;
    h.resolve=libc::RESOLVE_BENEATH|libc::RESOLVE_NO_SYMLINKS;
    let f=unsafe{File::from(kernel::new_fd(libc::syscall(libc::SYS_openat2,root.as_raw_fd(),c"proc".as_ptr(),&raw const h,
        size_of::<libc::open_how>()))?)};
    if !f.metadata().map_err(|_|Fault::Handoff)?.is_dir(){return Err(Fault::Handoff);}
    Ok(f)
}
fn view_flags(f:&File)->Result<libc::c_ulong>{
    let mut v:libc::statvfs=unsafe{zeroed()};
    if unsafe{libc::fstatvfs(f.as_raw_fd(),&raw mut v)}!=0{return Err(Fault::Handoff);}
    Ok(v.f_flag)
}
