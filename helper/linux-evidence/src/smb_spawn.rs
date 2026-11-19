//! Linux/amd64-only launch of the two fixed online roles; never a command runner.
//! The installation owner supplies verified, separately prepared private views.
//! clone3 returns the ORIGINAL pidfd atomically. Only this module constructs Born.
mod child;
use linux_raw_sys::general as uapi;
use rustix::{fd::AsFd,io::{Errno,fcntl_dupfd_cloexec},pipe::{PipeFlags,pipe_with},
    process::{PidfdFlags,Signal,WaitId,WaitIdOptions,getpid,pidfd_open,pidfd_send_signal,waitid}};
use std::{ffi::CString,fs::File,io::{self,Read},mem,os::fd::{AsRawFd,BorrowedFd,FromRawFd},
    ptr,thread,time::{Duration,Instant}};

fn bad()->io::Error{io::Error::other("dedicated service child or private launch inputs unavailable")}
pub struct StorageConfig{uid:u32,gid:u32,env:[CString;5]}
impl StorageConfig{
    pub fn selected(uid:u32,gid:u32,root:&str,pool:[u8;16],site:[u8;16],op:[u8;16],config:[u8;32])->io::Result<Self>{
        if [uid,gid].iter().any(|x|matches!(*x,0|65534|u32::MAX)) || root.len()>384 ||
            !(root.starts_with("/srv/")||root.starts_with("/mnt/")) || pool==[0;16]||site==[0;16]||op==[0;16]||config==[0;32] {return Err(bad());}
        let parts:Vec<_>=root[1..].split('/').collect();
        if parts.len()<2||parts.len()>17||parts.iter().any(|p|p.is_empty()||p.len()>63||
            !p.as_bytes()[0].is_ascii_alphanumeric()&&p.as_bytes()[0]!=b'_'||
            !p.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'_'|b'.'|b'-'))){return Err(bad());}
        let hex=|b:&[u8]|b.iter().map(|v|format!("{v:02x}")).collect::<String>();
        let texts=[format!("ZXS_SYSTEM_ROOT={root}"),format!("ZXS_POOL_ID={}",hex(&pool)),
            format!("ZXS_SAMBA_INSTANCE_ID={}",hex(&site)),format!("ZXS_SAMBA_PROVISION_OP={}",hex(&op)),
            format!("ZXS_SAMBA_LIVE_CONFIG_SHA256={}",hex(&config))];
        Ok(Self{uid,gid,env:texts.map(|s|CString::new(s).expect("validated ASCII"))})
    }
}
enum Role<'a>{Daemon,Storage(&'a StorageConfig)}
pub struct Born{original:File,ack:File,done:Option<i32>}
impl Born{
    pub fn original(&self)->&File{&self.original}
    /// The CLOEXEC error pipe is not by itself success (also EOF on death).
    /// The caller additionally checks this live child against the selected ELF.
    pub fn exec_barrier(&mut self,until:Instant)->io::Result<()>{
        let mut b=[0];loop{
            if Instant::now()>=until||self.peek()?.is_some(){return Err(bad());}
            match self.ack.read(&mut b){
                Ok(0)=>return if self.peek()?.is_none(){Ok(())}else{Err(bad())},Ok(_)=>return Err(bad()),
                Err(e) if matches!(e.kind(),io::ErrorKind::Interrupted|io::ErrorKind::WouldBlock)=>{},
                Err(e)=>return Err(e),
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    /// Peek does not reap or signal; only finish/abort own the actual wait.
    pub fn peek(&self)->io::Result<Option<i32>>{
        if let Some(n)=self.done{return Ok(Some(n));}
        loop{match waitid(WaitId::PidFd(self.original.as_fd()),WaitIdOptions::EXITED|WaitIdOptions::NOHANG|WaitIdOptions::NOWAIT){
            Ok(Some(s))=>return Ok(Some(s.exit_status().unwrap_or(-1))),Ok(None)=>return Ok(None),
            Err(Errno::INTR)=>continue,Err(e)=>return Err(e.into()),
        }}
    }
    pub fn term(&self)->io::Result<()>{
        if self.done.is_some(){return Err(bad());}
        pidfd_send_signal(&self.original,Signal::TERM)?;Ok(())
    }
    fn reap(&mut self)->io::Result<i32>{
        if let Some(n)=self.done{return Ok(n);}
        loop{match waitid(WaitId::PidFd(self.original.as_fd()),WaitIdOptions::EXITED){
            Ok(Some(s))=>{let n=s.exit_status().unwrap_or(-1);self.done=Some(n);return Ok(n);},
            Err(Errno::INTR)=>continue,Ok(None)=>return Err(bad()),Err(e)=>return Err(e.into()),
        }}
    }
    pub fn finish(&mut self)->io::Result<i32>{if self.peek()?.is_none(){return Err(bad());}self.reap()}
    /// Each role is PID 1 in its **own** child namespace; fatal shutdown of
    /// that original init entails the kernel's teardown of that namespace.
    /// This is NEVER a normal drain. A kernel-stalled task can delay wait.
    pub fn kill_original(&self)->io::Result<()>{
        if self.done.is_some(){return Ok(());}
        match pidfd_send_signal(&self.original,Signal::KILL){Ok(())|Err(Errno::SRCH)=>Ok(()),Err(e)=>Err(e.into())}
    }
    pub fn abort(&mut self)->io::Result<()>{self.kill_original()?;self.reap()?;Ok(())}
}
impl Drop for Born{fn drop(&mut self){let _=self.abort();}}

pub fn daemon(image:&File,view:&File,endpoint:BorrowedFd<'_>)->io::Result<Born>{spawn(Role::Daemon,image,view,&[endpoint])}
pub fn storage(image:&File,view:&File,endpoint:BorrowedFd<'_>,bound:&File,master:&Born,config:&StorageConfig)->io::Result<Born>{
    if master.peek()?.is_some(){return Err(bad());}
    spawn(Role::Storage(config),image,view,&[endpoint,bound.as_fd(),master.original.as_fd()])
}
fn high(fd:BorrowedFd<'_>)->io::Result<File>{Ok(File::from(fcntl_dupfd_cloexec(fd,16)?))}
#[allow(unsafe_code)]
fn spawn(role:Role<'_>,image:&File,view:&File,inputs:&[BorrowedFd<'_>])->io::Result<Born>{
    crate::diagnostic_process::require_waitable_children()?;
    child::only_null_stdio()?;
    let parent=pidfd_open(getpid(),PidfdFlags::empty())?;let parent=high(parent.as_fd())?;
    let image=high(image.as_fd())?;let view=high(view.as_fd())?;
    let fds=inputs.iter().map(|fd|high(*fd)).collect::<io::Result<Vec<_>>>()?;
    let (r,w)=pipe_with(PipeFlags::CLOEXEC|PipeFlags::NONBLOCK)?;
    let sink=high(w.as_fd())?;drop(w);
    let mut selected=[-1;3];for(dst,src)in selected.iter_mut().zip(&fds){*dst=src.as_raw_fd();}
    let argv:[*const libc::c_char;3];let mut env=[ptr::null();7];
    env[0]=c"LC_ALL=C".as_ptr();
    let id=match role{
        Role::Daemon=>{argv=[c"smbd".as_ptr(),c"--foreground".as_ptr(),ptr::null()];None}
        Role::Storage(c)=>{argv=[c"zxs-smb-storage".as_ptr(),ptr::null(),ptr::null()];
            for(dst,src)in env[1..].iter_mut().zip(&c.env){*dst=src.as_ptr();}Some((c.uid,c.gid))}
    };
    let mut pfd:libc::c_int=-1;
    // No shared VM/FD table, vfork, handler table, CLONE_PARENT or thread.
    // No library callback, heap operation or unwinding in the child branch.
    let mut args:uapi::clone_args=unsafe{mem::zeroed()};
    args.flags=(libc::CLONE_PIDFD|libc::CLONE_NEWPID)as u64;
    args.pidfd=(&raw mut pfd)as usize as u64;args.exit_signal=libc::SIGCHLD as u64;
    let rc=unsafe{libc::syscall(libc::SYS_clone3,&raw const args,mem::size_of_val(&args))};
    if rc<0{return Err(io::Error::last_os_error());}
    if rc==0{
        // All role parameters and C strings remain alive through the syscall;
        // no Rust destructor nor any caller-selected function runs here.
        child::exec(id,image.as_raw_fd(),view.as_raw_fd(),parent.as_raw_fd(),sink.as_raw_fd(),selected,&argv,&env)
    }
    drop(sink);
    // Linux successfully published a unique owned pidfd as part of *this*
    // clone before returning to its parent. Do not reopen the numeric rc.
    let original=unsafe{File::from_raw_fd(pfd)};
    Ok(Born{original,ack:File::from(r),done:None})
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn storage_has_only_five_fixed_nonsecret_installation_keys(){
        let c=StorageConfig::selected(2001,2002,"/srv/pool/S",[1;16],[2;16],[3;16],[4;32]).unwrap();
        assert_eq!(c.env[0].to_bytes(),b"ZXS_SYSTEM_ROOT=/srv/pool/S");
        assert_eq!(c.env[1].to_str().unwrap(),format!("ZXS_POOL_ID={}","01".repeat(16)));
        assert_eq!(c.env[4].to_str().unwrap(),format!("ZXS_SAMBA_LIVE_CONFIG_SHA256={}","04".repeat(32)));
        for (uid,gid,path) in [(0,42,"/srv/p"),(42,65534,"/mnt/p"),(42,42,"/srv/x/../p"),
            (42,42,"/srv/p/"),(42,42,"/var/v"),(42,42,"/srv/\nLD_PRELOAD")]{assert!(StorageConfig::selected(uid,gid,path,[1;16],[2;16],[3;16],[4;32]).is_err());}
    }
    // A private kernel-only fixture. No smbd, filesystem mount, user
    // credential, exec, network or durable volume. Run only when explicitly
    // enabled in a separate disposable privileged Debian test environment.
    #[test]
    #[ignore="needs an expressly prepared, disposable Linux 6.12 VM with clone3 and private PID namespaces"]
    #[allow(unsafe_code)]
    fn two_live_namespace_inits_are_reaped_by_their_own_birth_pidfds(){
        if std::env::var_os("HELPER_TEST_PRIVATE_PID_NAMESPACE").as_deref()!=Some(std::ffi::OsStr::new("yes")){panic!("dedicated test VM required");}
        fn fixture()->(Born,File){
            let (r,w)=pipe_with(PipeFlags::CLOEXEC).unwrap();let (ack,signal)=pipe_with(PipeFlags::NONBLOCK|PipeFlags::CLOEXEC).unwrap();
            let mut fd=-1i32;let mut args:uapi::clone_args=unsafe{mem::zeroed()};
            args.flags=(libc::CLONE_PIDFD|libc::CLONE_NEWPID) as u64;args.pidfd=(&raw mut fd)as usize as u64;args.exit_signal=libc::SIGCHLD as u64;
            let rc=unsafe{libc::syscall(libc::SYS_clone3,&raw const args,mem::size_of_val(&args))};
            assert!(rc>=0);
            if rc==0{unsafe{
                libc::close(w.as_raw_fd());libc::close(signal.as_raw_fd());let mut n=0u8;
                let ok=libc::getpid()==1&&libc::read(r.as_raw_fd(),(&raw mut n).cast(),1)==1&&n==7;
                libc::_exit(if ok{7}else{125});
            }}
            drop(signal);drop(r);
            (Born{original:unsafe{File::from_raw_fd(fd)},ack:File::from(ack),done:None},File::from(w))
        }
        use std::io::Write;
        let(mut first,mut complete)=fixture();let(mut second,_other)=fixture();
        first.exec_barrier(Instant::now()+Duration::from_secs(2)).unwrap();assert!(first.peek().unwrap().is_none());
        second.abort().unwrap();assert_eq!(second.finish().unwrap(),-1);
        assert!(first.peek().unwrap().is_none());complete.write_all(&[7]).unwrap();
        let end=Instant::now()+Duration::from_secs(2);while first.peek().unwrap().is_none(){assert!(Instant::now()<end);thread::yield_now();}
        assert_eq!(first.finish().unwrap(),7);
    }
}
