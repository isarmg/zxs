//! Shared fixed one-shot original transport for both same-ELF low roles.
//! Different role frames and public payloads are still decoded separately;
//! this code never selects credentials, plans, listeners or paths.
use super::{BootSocket,Control,Fault,KernelControl,MAX_RIGHTS,OriginalProcess,Result,SEALS,START_BYTES,Supervisor,
    CONTROL_SIZE,cloexec,get_flags,inherit,initial_child,recv,sock,sock_int};
use std::{
    fs::File,io::{self,Write},mem::{size_of,zeroed},net::{Ipv4Addr,SocketAddr,TcpListener},
    os::fd::{AsRawFd,BorrowedFd,FromRawFd,OwnedFd,RawFd},os::unix::fs::{FileExt,MetadataExt},
    time::Instant,
};
const PUBLIC_MAX:usize=300*1024;

#[must_use]
pub(in crate::smb::launch) struct InheritedControl{
    original:Supervisor,
    subject:(u32,u32),
}
#[must_use]
pub(in crate::smb::launch) struct VerifiedControl{
    original:Supervisor,
    subject:(u32,u32),
}
#[must_use]
pub(in crate::smb::launch) struct ReceivedOriginals(Control);
impl InheritedControl{
    /// Only the fixed low process entry may consume its original fd3/fd4.
    ///
    /// # Safety
    /// Call in the dedicated role once, before threads and with no other
    /// owners/users of these descriptors. It takes both on success.
    pub(in crate::smb::launch) unsafe fn inherit()->Result<Self>{
        let subject=initial_child()?;
        // SAFETY: the role entry relinquishes these original descriptors.
        let (socket,parent)=unsafe{(inherit(3)?,File::from(inherit(4)?))};
        let original=Supervisor{socket:BootSocket::pin(socket)?,parent:OriginalProcess::pin(parent)?};
        original.require_live()?;
        Ok(Self{original,subject})
    }
    pub(super) fn recv_server(self,until:Instant)->Result<(VerifiedControl,[u8;START_BYTES],ReceivedOriginals)>{
        self.receive::<START_BYTES>(until,4)
    }
    #[allow(dead_code)] // called by the paired fixed internal Web entry
    pub(in crate::smb::launch) fn recv_web(self,until:Instant)
        ->Result<(VerifiedControl,[u8;START_BYTES],ReceivedOriginals)>{
        self.receive::<START_BYTES>(until,3)
    }
    #[allow(dead_code)] // used by the paired standalone preflight source
    pub(in crate::smb::launch) fn recv_once<const N:usize>(self,until:Instant)
        ->Result<(VerifiedControl,[u8;N],ReceivedOriginals)>{
        self.receive(until,1)
    }
    fn receive<const N:usize>(self,until:Instant,minimum:usize)
        ->Result<(VerifiedControl,[u8;N],ReceivedOriginals)>{
        if N!=START_BYTES{return Err(Fault::Handoff);}
        let fd=self.original.socket.as_raw_fd();
        let mut packet=[0;N];
        let (n,mut control)=recv(fd,&mut packet,until)?;
        // Ancestor UID/GID must be real kernel root in the initial user
        // namespace; translation to PID zero alone is not identity.
        if n!=N||control.bad||control.nrights<minimum||control.credentials!=Some((0,0,0)){
            return Err(Fault::Handoff);
        }
        self.original.parent.same_sender(File::from(control.pidfd.take().ok_or(Fault::Handoff)?))?;
        self.original.require_live()?;
        let (n,finish)=recv(fd,&mut [0;1],until)?;
        if n!=0||!finish.empty(){return Err(Fault::Handoff);} // original half-close, never a second packet
        self.original.require_live()?;
        let peer=VerifiedControl{original:self.original,subject:self.subject};
        Ok((peer,packet,ReceivedOriginals(control)))
    }
}
impl VerifiedControl{
    pub(in crate::smb::launch) fn subject(&self)->(u32,u32){self.subject}
    pub(super) fn into_supervisor(self)->Supervisor{self.original}
    #[allow(dead_code)]
    pub(in crate::smb::launch) fn require_live(&self)->Result<()>{self.original.require_live()}
    #[allow(dead_code)]
    pub(in crate::smb::launch) fn send_sealed_reply(self,frame:&[u8;START_BYTES],sealed:&File,until:Instant)->Result<()>{
        self.require_live()?;
        sealed_size(sealed,PUBLIC_MAX)?;
        if Instant::now()>=until{return Err(Fault::Handoff);}
        send_packet(&self.original.socket,frame,&[sealed.as_raw_fd()],||Ok(()))?;
        self.require_live()?;
        if Instant::now()>=until{return Err(Fault::Handoff);}
        Ok(())
    }
    /// Only the actual Web entry, after its independently qualified S,
    /// server listener and normal stop handlers. Hold the original parent
    /// liveness through that Web's normal HTTP workers and final drain.
    #[allow(dead_code)]
    pub(in crate::smb::launch) fn publish_web_prepared(self,frame:&[u8;START_BYTES],until:Instant)->Result<VerifiedWebRun>{
        self.require_live()?;
        if &frame[..16]!=super::super::web::PREPARED||Instant::now()>=until{return Err(Fault::Handoff);}
        let fd=self.original.socket.as_raw_fd();
        // SAFETY: exact own original socket and fixed live frame. Send once
        // with no SCM_RIGHTS; failure cannot advance a separate READY path.
        if unsafe{libc::send(fd,frame.as_ptr().cast(),frame.len(),libc::MSG_NOSIGNAL|libc::MSG_DONTWAIT)}
            !=frame.len() as isize||unsafe{libc::shutdown(fd,libc::SHUT_WR)}!=0{
            return Err(Fault::Handoff);
        }
        self.require_live()?;
        if Instant::now()>=until{return Err(Fault::Handoff);}
        Ok(VerifiedWebRun{original:self.original})
    }
}
#[must_use]
pub(in crate::smb::launch) struct VerifiedWebRun{original:Supervisor}
impl VerifiedWebRun{
    #[allow(dead_code)]
    pub(in crate::smb::launch) fn require_live(&self)->Result<()>{self.original.require_live()}
}
#[must_use]
pub(in crate::smb::launch) struct UnverifiedWebListener(OwnedFd);
impl UnverifiedWebListener{
    #[allow(dead_code)]
    pub(in crate::smb::launch) fn match_cookie(self,cookie:u64)->Result<TcpListener>{
        require_web_socket(&self.0,cookie)?;
        let listener=TcpListener::from(self.0);
        if listener.local_addr().map_err(|_|Fault::Handoff)?!=SocketAddr::from((Ipv4Addr::UNSPECIFIED,8443)){
            return Err(Fault::Handoff);
        }
        Ok(listener)
    }
}
#[allow(dead_code)]
fn require_web_socket(fd:&OwnedFd,cookie:u64)->Result<()>{
    let fd=fd.as_raw_fd();
    cloexec(fd)?;
    if cookie==0||u64::from_ne_bytes(sock(fd,libc::SOL_SOCKET,libc::SO_COOKIE)?)!=cookie||
        get_flags(fd,libc::F_GETFL)?&libc::O_NONBLOCK==0||
        sock_int(fd,libc::SOL_SOCKET,libc::SO_DOMAIN)?!=libc::AF_INET||
        sock_int(fd,libc::SOL_SOCKET,libc::SO_TYPE)?!=libc::SOCK_STREAM||
        sock_int(fd,libc::SOL_SOCKET,libc::SO_PROTOCOL)?!=libc::IPPROTO_TCP||
        sock_int(fd,libc::SOL_SOCKET,libc::SO_REUSEPORT)?!=0||
        sock_int(fd,libc::SOL_SOCKET,libc::SO_ACCEPTCONN)?!=1{
        return Err(Fault::Handoff);
    }
    Ok(())
}
impl ReceivedOriginals{
    pub(super) fn count(&self)->usize{self.0.nrights}
    pub(super) fn take(&mut self,index:usize)->Result<OwnedFd>{
        self.0.rights.get_mut(index).and_then(Option::take).ok_or(Fault::Handoff)
    }
    #[allow(dead_code)]
    pub(in crate::smb::launch) fn only_sealed<const MAX:usize>(mut self)->Result<Vec<u8>>{
        if self.count()!=1{return Err(Fault::Handoff);}
        read_sealed(&File::from(self.take(0)?),MAX)
    }
    #[allow(dead_code)]
    pub(in crate::smb::launch) fn only_web<const MAX:usize>(mut self)->Result<(Vec<u8>,UnverifiedWebListener,OwnedFd)>{
        if self.count()!=3{return Err(Fault::Handoff);}
        let bytes=read_sealed(&File::from(self.take(0)?),MAX)?;
        Ok((bytes,UnverifiedWebListener(self.take(1)?),self.take(2)?))
    }
}
fn sealed_size(file:&File,maximum:usize)->Result<usize>{
    let fd=file.as_raw_fd();cloexec(fd)?;
    let seals=get_flags(fd,libc::F_GET_SEALS)?;
    let m=file.metadata().map_err(|_|Fault::Handoff)?;
    if !m.is_file()||m.nlink()!=0||m.mode()&0o111!=0||seals&SEALS!=SEALS{return Err(Fault::Handoff);}
    usize::try_from(m.len()).ok().filter(|n|(1..=maximum.min(PUBLIC_MAX)).contains(n)).ok_or(Fault::Handoff)
}
pub(in crate::smb::launch) fn read_sealed(file:&File,maximum:usize)->Result<Vec<u8>>{
    let size=sealed_size(file,maximum)?;
    let mut b=Vec::new();b.try_reserve_exact(size).map_err(|_|Fault::Resources)?;b.resize(size,0);
    file.read_exact_at(&mut b,0).map_err(|_|Fault::Handoff)?;
    loop{
        match file.read_at(&mut [0;1],size as u64){
            Ok(0)=>return Ok(b),
            Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,
            _=>return Err(Fault::Handoff),
        }
    }
}
#[allow(dead_code)]
pub(in crate::smb::launch) fn seal_public(bytes:&[u8],maximum:usize)->Result<File>{
    if !(1..=maximum.min(PUBLIC_MAX)).contains(&bytes.len()){return Err(Fault::Shape);}
    // SAFETY: create one anonymous nonexecutable memory file; fixed C name.
    let raw=unsafe{libc::memfd_create(c"zxs-smb-public".as_ptr(),libc::MFD_CLOEXEC|libc::MFD_ALLOW_SEALING|libc::MFD_NOEXEC_SEAL)};
    if raw<0{return Err(Fault::Handoff);}
    // SAFETY: the kernel gave this sole fresh descriptor.
    let mut file=unsafe{File::from_raw_fd(raw)};
    file.write_all(bytes).map_err(|_|Fault::Handoff)?;
    // SAFETY: command takes the fixed integer seal flags on this owned fd.
    if unsafe{libc::fcntl(raw,libc::F_ADD_SEALS,SEALS)}!=0||sealed_size(&file,maximum)?!=bytes.len(){return Err(Fault::Handoff);}
    Ok(file)
}
fn send_packet(socket:&BootSocket,frame:&[u8;START_BYTES],fds:&[RawFd],before:impl FnOnce()->Result<()>)->Result<()>{
    if !(1..=MAX_RIGHTS).contains(&fds.len()){return Err(Fault::Handoff);}
    for &fd in fds{cloexec(fd)?;}
    let mut control=KernelControl{_align:[],bytes:[0;CONTROL_SIZE]};
    let mut data=libc::iovec{iov_base:frame.as_ptr().cast_mut().cast(),iov_len:frame.len()};
    // SAFETY: initialized C descriptor and bounded, live data/control spans.
    let mut msg:libc::msghdr=unsafe{zeroed()};
    msg.msg_iov=&raw mut data;msg.msg_iovlen=1;msg.msg_control=control.bytes.as_mut_ptr().cast();
    let bytes=(fds.len()*size_of::<RawFd>()) as libc::c_uint;
    // SAFETY: the aligned control fits every validated fd in one SCM_RIGHTS
    // message.
    unsafe{
        msg.msg_controllen=libc::CMSG_SPACE(bytes) as _;
        let h=libc::CMSG_FIRSTHDR(&msg);
        if h.is_null(){return Err(Fault::Handoff);}
        (*h).cmsg_level=libc::SOL_SOCKET;(*h).cmsg_type=libc::SCM_RIGHTS;(*h).cmsg_len=libc::CMSG_LEN(bytes) as _;
        std::ptr::copy_nonoverlapping(fds.as_ptr().cast::<u8>(),libc::CMSG_DATA(h),bytes as usize);
    }
    before()?; // the caller's nonblocking final vote, after all local FD checks
    let fd=socket.as_raw_fd();
    // SAFETY: same complete, bounded and still-retained C message. Send
    // once; never retry an unknown short write or emit a second packet.
    unsafe{
        if libc::sendmsg(fd,&raw const msg,libc::MSG_DONTWAIT|libc::MSG_NOSIGNAL)!=frame.len() as isize
            ||libc::shutdown(fd,libc::SHUT_WR)!=0{return Err(Fault::Handoff);}
    }
    Ok(())
}

/// Parent may send only after custody has proved the exact born child and
/// the caller has separately acquired the real S/site/approval proofs.
#[allow(dead_code)]
pub(in crate::smb::launch) fn offer_packet(socket:&BootSocket,frame:&[u8;START_BYTES],public:&File,
    additional:&[BorrowedFd<'_>],until:Instant,before:impl FnOnce()->Result<()>)->Result<()>{
    sealed_size(public,PUBLIC_MAX)?;
    if additional.len()>=MAX_RIGHTS||Instant::now()>=until{return Err(Fault::Handoff);}
    let mut fds=[-1;MAX_RIGHTS];fds[0]=public.as_raw_fd();
    for (dst,src) in fds[1..].iter_mut().zip(additional){*dst=src.as_raw_fd();}
    send_packet(socket,frame,&fds[..1+additional.len()],||{
        if Instant::now()>=until{return Err(Fault::Handoff);}
        before()
    })
}

/// The same original birth that received START must emit one kernel
/// record and its write-half EOF. A preflight child can already be a
/// waitable zombie with a completed reply; its parent still owns reaping.
#[allow(dead_code)]
pub(in crate::smb::launch) fn child_receipt(socket:&BootSocket,original:&OriginalProcess,
    subject:(u32,u32),sealed:bool,until:Instant)->Result<([u8;START_BYTES],Option<File>)>{
    let fd=socket.as_raw_fd();
    let mut frame=[0;START_BYTES];
    let (n,mut c)=recv(fd,&mut frame,until)?;
    if n!=START_BYTES||c.bad||c.nrights!=sealed as usize||c.seen_rights!=sealed||
        !c.credentials.is_some_and(|(pid,uid,gid)|pid>1&&(uid,gid)==subject){return Err(Fault::Handoff);}
    original.same_sender(File::from(c.pidfd.take().ok_or(Fault::Handoff)?))?;
    let (n,end)=recv(fd,&mut [0;1],until)?;
    if n!=0||!end.empty(){return Err(Fault::Handoff);}
    let public=c.rights[0].take().map(File::from);
    if let Some(file)=&public{sealed_size(file,PUBLIC_MAX)?;}
    Ok((frame,public))
}
