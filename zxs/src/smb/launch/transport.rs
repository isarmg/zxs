//! Linux boundary for the original supervisor START/READY transport. Ready
//! is not a substitute for actual Core or the protected verifier.
mod vault;
mod accept;
mod ready;
mod handshake;
pub(in crate::smb) use accept::OwnedAccepted;
#[allow(unused_imports)] // the standalone low preflight is added separately
pub(super) use handshake::{InheritedControl,child_receipt,offer_packet,read_sealed,seal_public};
use super::{DIRECT_BIND,Fault,Result,descriptor::{self,Access,cloexec},
    envelope::{ENV_MAX,Envelope,START_BYTES,Start,VaultStamp},
    held_open::{self,ProcOpen},
    kernel::{BootSocket,OriginalProcess,fs_magic},socket_facts::{self,Integer}};
use crate::smb::readiness::{self,Interest,Watch};
use std::{
    fs::{File,OpenOptions},io,mem::{size_of,zeroed},net::TcpListener,
    os::fd::{AsFd,AsRawFd,BorrowedFd,FromRawFd,OwnedFd,RawFd},os::unix::fs::{MetadataExt,OpenOptionsExt},
    path::Path,sync::{Mutex,atomic::AtomicU8},time::{Duration,Instant},
};

const SCM_PIDFD:libc::c_int=4; // Linux v6.12 include/linux/socket.h
const WAIT:Duration=Duration::from_secs(10);
const MAX_RIGHTS:usize=4;
const CONTROL_SIZE:usize=unsafe{
    libc::CMSG_SPACE((MAX_RIGHTS*size_of::<RawFd>()) as u32) as usize+
    libc::CMSG_SPACE(size_of::<libc::ucred>() as u32) as usize+
    libc::CMSG_SPACE(size_of::<RawFd>() as u32) as usize
};
const CMSG_HEAD:usize=unsafe{libc::CMSG_LEN(0) as usize};
const SEALS:libc::c_int=libc::F_SEAL_WRITE|libc::F_SEAL_GROW|libc::F_SEAL_SHRINK|libc::F_SEAL_SEAL|libc::F_SEAL_EXEC;

struct Supervisor {socket:BootSocket,parent:OriginalProcess}
struct VaultInput {file:File,stamp:VaultStamp}

/// Kernel-attested launch owner, not a storage, authentication or serving
/// grant. Core may borrow only nonsecret comparison facts from this owner.
pub(crate) struct InheritedRun {
    supervisor:Supervisor,
    start:Start,
    until:Instant,
    ready:AtomicU8,
    env:Envelope,
    pair:Mutex<Option<super::pair_gate::WaitingGate>>,
    vault:Mutex<Option<VaultInput>>,
    listener:TcpListener,
}
impl InheritedRun {
    pub(crate) fn system_path(&self)->&Path{Path::new(&self.env.system)}
    pub(crate) fn system_entity(&self)->crate::fs::Entity{
        crate::fs::Entity{device:self.env.sdev,inode:self.env.sino}
    }
    pub(crate) fn rust_plan(&self)->&[u8]{&self.env.plan}
    pub(crate) fn release_sha(&self)->[u8;32]{self.env.release_sha}
    pub(crate) fn current_public_revision_is(&self,account:[u8;16],credential:[u8;16],record:[u8;16],
        sid:&[u8;28],generation:i64,login:&str)->bool{
        let e=&self.env;
        e.account==account&&e.credential==credential&&e.record==record&&
            &e.sid==sid&&e.generation==generation&&e.login==login
    }
    pub(crate) fn require_live_source(&self)->io::Result<()>{
        self.supervisor.require_live().map_err(|_|io::Error::new(io::ErrorKind::PermissionDenied,
            "the original Rust SMB supervisor is not live"))
    }
    pub(crate) fn require_listener_binding(&self)->io::Result<()>{
        if self.listener.local_addr()?!=DIRECT_BIND{
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(())
    }

    /// One call in the dedicated child, before any worker or other users of
    /// the fixed launch descriptors. The only binary caller is the
    /// dedicated Rust SMB worker, never the Web or native service.
    ///
    /// # Safety
    /// Inherited descriptors 3 and 4 must have no other owners or users
    /// inside this process. The dedicated process entry transfers them.
    pub(super) unsafe fn receive()->Result<Self>{
        // SAFETY: the dedicated entry relinquishes exclusive fd3/fd4.
        let link=unsafe{InheritedControl::inherit()}?;
        let until=Instant::now().checked_add(WAIT).ok_or(Fault::Handoff)?;
        let (verified,packet,mut control)=link.recv_server(until)?;
        let (uid,gid)=verified.subject();
        super::low::require_reduced((uid,gid))?;
        let supervisor=verified.into_supervisor();
        let start=Start::decode(&packet)?;
        let sealed=File::from(control.take(0)?);
        let env=start.verify(&frozen(&sealed)?)?;
        drop(sealed);
        if (uid,gid)!=(env.uid,env.gid)||control.count()!=MAX_RIGHTS{return Err(Fault::Handoff);}
        let file=File::from(control.take(1)?);
        let stamp=env.vault;
        vault_file(&file,&stamp)?;
        let listener=listener(control.take(2)?,env.listener_cookie)?;
        let pair=super::pair_gate::WaitingGate::from_start(control.take(3)?,
            env.pair.gate,env.pair.nonce,env.pair.cohort)?;
        supervisor.require_live()?;
        Ok(Self{supervisor,start,until,ready:AtomicU8::new(0),env,pair:Mutex::new(Some(pair)),vault:Mutex::new(Some(VaultInput{file,stamp})),listener})
    }
}
impl Supervisor {
    fn require_live(&self)->Result<()>{self.parent.require_live()}
}
fn initial_child()->Result<(u32,u32)>{
    // An apparent root UID in a caller-created user namespace is not host
    // root. Pin the actual *current task* link without following its target
    // or crossing a submount. Harmless untrusted filesystem text never wins.
    let proc=OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY|libc::O_NOFOLLOW)
        .open("/proc").map_err(|_|Fault::Handoff)?;
    if fs_magic(proc.as_raw_fd())?!=libc::PROC_SUPER_MAGIC as libc::c_long||
        proc.metadata().map_err(|_|Fault::Handoff)?.ino()!=1{return Err(Fault::Handoff);}
    let link=held_open::proc(&proc,c"thread-self/ns/user",ProcOpen::Pin).map_err(|_|Fault::Handoff)?;
    let mut buf=[0u8;40];
    // SAFETY: the O_PATH symlink and writable output span are retained.
    let n=unsafe{libc::readlinkat(link.as_raw_fd(),c"".as_ptr(),buf.as_mut_ptr().cast(),buf.len())};
    let expect=b"user:[4026531837]"; // PROC_USER_INIT_INO, Linux include/linux/proc_ns.h
    if n!=expect.len() as isize||&buf[..expect.len()]!=expect{return Err(Fault::Handoff);}
    // SAFETY: read local process credentials.
    let (uid,gid,r_uid,r_gid,pid,nnp)=unsafe{(libc::geteuid(),libc::getegid(),libc::getuid(),libc::getgid(),
        libc::getpid(),libc::prctl(libc::PR_GET_NO_NEW_PRIVS,0 as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong))};
    if matches!(uid,0|65534|u32::MAX)||matches!(gid,0|65534|u32::MAX)||uid!=r_uid||gid!=r_gid||pid!=1||nnp!=1{
        return Err(Fault::Handoff);
    }
    // exec may reset dumpability. Do this before receiving any protected FD.
    // SAFETY: process-local hardening in the single-threaded child entry.
    if unsafe{libc::prctl(libc::PR_SET_DUMPABLE,0 as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong)}!=0{
        return Err(Fault::Handoff);
    }
    Ok((uid,gid))
}
unsafe fn inherit(fd:RawFd)->Result<OwnedFd>{
    let flags=descriptor::flags(fd)?.native_bits();
    // SAFETY: the isolated caller owns the fixed live descriptor exactly once.
    let own=unsafe{OwnedFd::from_raw_fd(fd)};
    // SAFETY: F_SETFD takes this integer argument for the retained fd.
    if unsafe{libc::fcntl(fd,libc::F_SETFD,flags|libc::FD_CLOEXEC)}!=0{return Err(Fault::Handoff);}
    Ok(own)
}
fn listener(fd:OwnedFd,cookie:u64)->Result<TcpListener>{
    let socket=fd.as_fd();let raw=socket.as_raw_fd();cloexec(raw)?;
    if !descriptor::status(raw)?.nonblocking(){return Err(Fault::Handoff);}
    if socket_facts::integer(socket,Integer::Domain)?!=libc::AF_INET||
        socket_facts::integer(socket,Integer::SocketType)?!=libc::SOCK_STREAM||
        socket_facts::integer(socket,Integer::Protocol)?!=libc::IPPROTO_TCP||
        socket_facts::integer(socket,Integer::Accepting)?!=1||
        socket_facts::integer(socket,Integer::ReusePort)?!=0||
        socket_facts::cookie(socket)?!=cookie{return Err(Fault::Handoff);}
    let listener=TcpListener::from(fd);
    if listener.local_addr().map_err(|_|Fault::Handoff)?!=DIRECT_BIND{return Err(Fault::Handoff);}
    Ok(listener)
}
fn frozen(file:&File)->Result<Vec<u8>>{handshake::read_sealed(file,ENV_MAX)}
fn vault_file(file:&File,s:&VaultStamp)->Result<()>{
    let fd=file.as_raw_fd();cloexec(fd)?;
    if descriptor::status(fd)?.access()!=Some(Access::ReadOnly){return Err(Fault::Handoff);}
    let m=file.metadata().map_err(|_|Fault::Handoff)?;
    if !m.is_file()||m.mode()&0o7777!=0o600||m.uid()!=0||m.gid()!=0||m.nlink()!=1||m.len()!=240||
        m.dev()!=s.dev||m.ino()!=s.ino||m.ctime()!=s.ctime_s||m.ctime_nsec()!=i64::from(s.ctime_ns)||
        m.mtime()!=s.mtime_s||m.mtime_nsec()!=i64::from(s.mtime_ns){return Err(Fault::Handoff);}
    Ok(())
}

#[repr(C)]
struct KernelControl{_align:[libc::cmsghdr;0],bytes:[u8;CONTROL_SIZE]}
struct Control {
    rights:[Option<OwnedFd>;MAX_RIGHTS],nrights:usize,pidfd:Option<OwnedFd>,
    credentials:Option<(libc::pid_t,libc::uid_t,libc::gid_t)>,seen_rights:bool,bad:bool,
}
impl Control {
    fn empty(&self)->bool{!self.bad&&!self.seen_rights&&self.pidfd.is_none()&&self.credentials.is_none()}
    /// # Safety
    /// The input is only output from a successful Linux recvmsg in this
    /// process. The kernel has just installed every received fd number.
    unsafe fn take_kernel(mut raw:&[u8])->Self{
        let mut s=Self{rights:std::array::from_fn(|_|None),nrights:0,pidfd:None,credentials:None,seen_rights:false,bad:false};
        while !raw.is_empty(){
            if raw.len()<CMSG_HEAD{s.bad=true;break;}
            // SAFETY: the integer C header fits in the successful byte span.
            let h=unsafe{raw.as_ptr().cast::<libc::cmsghdr>().read_unaligned()};
            let n=h.cmsg_len as usize;
            if !(CMSG_HEAD..=raw.len()).contains(&n){s.bad=true;break;}
            let data=&raw[CMSG_HEAD..n];
            match (h.cmsg_level,h.cmsg_type){
                (libc::SOL_SOCKET,libc::SCM_RIGHTS|SCM_PIDFD)=>{
                    let is_right=h.cmsg_type==libc::SCM_RIGHTS;
                    if is_right{s.bad|=s.seen_rights;s.seen_rights=true;}
                    else{s.bad|=data.len()!=size_of::<RawFd>();}
                    let (words,tail)=data.as_chunks::<{size_of::<RawFd>()}>();
                    for word in words{
                        let raw=RawFd::from_ne_bytes(*word);
                        if raw<0{s.bad=true;continue;}
                        // SAFETY: even denied cmsgs must acquire/destroy every
                        // freshly installed kernel fd. This never reads raw user bytes.
                        let fd=unsafe{OwnedFd::from_raw_fd(raw)};
                        if !is_right{
                            if s.pidfd.is_some(){s.bad=true;drop(fd);}else{s.pidfd=Some(fd);}
                        }else if let Some(slot)=s.rights.get_mut(s.nrights){
                            *slot=Some(fd);s.nrights+=1;
                        }else{s.bad=true;drop(fd);}
                    }
                    s.bad|=data.is_empty()||!tail.is_empty();
                }
                (libc::SOL_SOCKET,libc::SCM_CREDENTIALS) if data.len()==size_of::<libc::ucred>()=>{
                    // SAFETY: the complete C integer record came from the kernel.
                    let c=unsafe{data.as_ptr().cast::<libc::ucred>().read_unaligned()};
                    if s.credentials.replace((c.pid,c.uid,c.gid)).is_some(){s.bad=true;}
                }
                _=>s.bad=true,
            }
            // The last message's alignment may be partly outside the
            // supplied capacity even when its full header/payload fits.
            let pad=unsafe{libc::CMSG_SPACE(data.len() as u32)} as usize;
            raw=&raw[pad.min(raw.len())..];
        }
        s
    }
}
fn recv(fd:BorrowedFd<'_>,buf:&mut [u8],end:Instant)->Result<(usize,Control)>{
    loop{
        let left=end.saturating_duration_since(Instant::now());if left.is_zero(){return Err(Fault::Handoff);}
        let mut w=[Watch::new(fd,Interest::Input)];
        let ms=left.as_millis().saturating_add(1).min(i32::MAX as u128) as i32;
        match readiness::wait(&mut w,ms){
            Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,
            Ok(n) if n>0&&!w[0].events().invalid_or_error()=>(),
            _=>return Err(Fault::Handoff),
        }
        let mut c=KernelControl{_align:[],bytes:[0;CONTROL_SIZE]};
        let mut io=libc::iovec{iov_base:buf.as_mut_ptr().cast(),iov_len:buf.len()};
        // SAFETY: initialize the C header and specify live bounded spans.
        let mut m:libc::msghdr=unsafe{zeroed()};
        m.msg_control=c.bytes.as_mut_ptr().cast();m.msg_controllen=CONTROL_SIZE as _;
        m.msg_iov=&mut io;m.msg_iovlen=1;
        // SAFETY: the character and aligned control spans live across this receive.
        let n=unsafe{libc::recvmsg(fd.as_raw_fd(),&mut m,libc::MSG_CMSG_CLOEXEC|libc::MSG_DONTWAIT)};
        if n<0{
            if matches!(io::Error::last_os_error().kind(),io::ErrorKind::Interrupted|io::ErrorKind::WouldBlock){continue;}
            return Err(Fault::Handoff);
        }
        let used=m.msg_controllen as usize;
        // SAFETY: initialized output bytes of this successful kernel receive.
        let mut c=unsafe{Control::take_kernel(&c.bytes[..used.min(CONTROL_SIZE)])};
        c.bad|=used>CONTROL_SIZE||n as usize>buf.len()||
            m.msg_flags&(libc::MSG_TRUNC|libc::MSG_CTRUNC)!=0||Instant::now()>=end;
        return Ok((n as usize,c));
    }
}

#[cfg(test)]
#[path="../../../tests/smb/rust_launch_transport.rs"]
mod tests;
