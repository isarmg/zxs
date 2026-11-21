//! Anonymous synthetic fixture FDs, public bytes, no 445/network, vault,
//! low-privilege launch, or process state changes.
use super::*;
use std::io::Write;

fn public(bytes:&[u8],seals:libc::c_int)->File{
    // SAFETY: fixed C name and known process-local allocation flags.
    let fd=unsafe{libc::memfd_create(c"zxs-public-fixture".as_ptr(),libc::MFD_CLOEXEC|libc::MFD_ALLOW_SEALING|libc::MFD_NOEXEC_SEAL)};
    assert!(fd>=0);
    // SAFETY: the successful call returned a fresh fd.
    let mut file=unsafe{File::from_raw_fd(fd)};
    file.write_all(bytes).unwrap();
    if seals!=0{
        // SAFETY: F_ADD_SEALS uses the stated integer flags.
        assert_eq!(unsafe{libc::fcntl(fd,libc::F_ADD_SEALS,seals)},0);
    }
    file
}
#[test]
fn public_envelope_needs_the_actual_immutable_memfd(){
    let raw=b"only public test bytes";
    assert_eq!(frozen(&public(raw,SEALS)).unwrap().as_slice(),&raw[..]);
    assert_eq!(frozen(&public(raw,0)),Err(Fault::Handoff));
    assert_eq!(frozen(&public(raw,SEALS&!libc::F_SEAL_WRITE)),Err(Fault::Handoff));
    assert_eq!(frozen(&public(&[0;ENV_MAX+1],SEALS)),Err(Fault::Handoff));
}
fn local_pair()->(OwnedFd,OwnedFd){
    let mut pair=[-1;2];
    // SAFETY: initialized writable output array and private local fixture.
    assert_eq!(unsafe{libc::socketpair(libc::AF_UNIX,libc::SOCK_SEQPACKET|libc::SOCK_CLOEXEC,0,pair.as_mut_ptr())},0);
    let [a,b]=pair;
    // SAFETY: the successful call transferred unique owned fds.
    let (a,b)=unsafe{(OwnedFd::from_raw_fd(a),OwnedFd::from_raw_fd(b))};
    for opt in [libc::SO_PASSCRED,libc::SO_PASSPIDFD]{
        let yes:libc::c_int=1;
        // SAFETY: integer option and an initialized input span for it.
        assert_eq!(unsafe{libc::setsockopt(b.as_raw_fd(),libc::SOL_SOCKET,opt,
            (&yes as *const libc::c_int).cast(),size_of::<libc::c_int>() as libc::socklen_t)},0);
    }
    (a,b)
}
fn send(socket:&impl AsRawFd,file:&File,payload:&[u8]){
    let mut a=KernelControl{_align:[],bytes:[0;CONTROL_SIZE]};
    let mut data=libc::iovec{iov_base:payload.as_ptr().cast_mut().cast(),iov_len:payload.len()};
    // SAFETY: null/zero initial pointer and integer members.
    let mut m:libc::msghdr=unsafe{zeroed()};
    m.msg_iov=&mut data;m.msg_iovlen=1;m.msg_control=a.bytes.as_mut_ptr().cast();
    // SAFETY: aligned writable area for one CMSG with an exact fd payload.
    unsafe{
        m.msg_controllen=libc::CMSG_SPACE(size_of::<RawFd>() as u32) as _;
        let h=libc::CMSG_FIRSTHDR(&m);assert!(!h.is_null());
        (*h).cmsg_level=libc::SOL_SOCKET;(*h).cmsg_type=libc::SCM_RIGHTS;
        (*h).cmsg_len=libc::CMSG_LEN(size_of::<RawFd>() as u32) as _;
        libc::CMSG_DATA(h).cast::<RawFd>().write_unaligned(file.as_raw_fd());
        assert_eq!(libc::sendmsg(socket.as_raw_fd(),&m,libc::MSG_NOSIGNAL),payload.len() as isize);
    }
}
#[test]
fn kernel_supplies_rights_and_pidfd_and_true_eof_has_no_control(){
    let (a,b)=local_pair();let dummy=public(b"not a vault",SEALS);
    send(&a,&dummy,&[1u8;START_BYTES]);
    // SAFETY: half-close the fixture, not an application stream.
    assert_eq!(unsafe{libc::shutdown(a.as_raw_fd(),libc::SHUT_WR)},0);
    let until=Instant::now()+WAIT;
    let (n,mut c)=recv(b.as_raw_fd(),&mut [0;START_BYTES],until).unwrap();
    assert_eq!(n,START_BYTES);assert!(!c.bad);assert_eq!(c.nrights,1);
    // SAFETY: local fixture credentials.
    assert_eq!(c.credentials,Some(unsafe{(libc::getpid(),libc::getuid(),libc::getgid())}));
    let pid=File::from(c.pidfd.take().unwrap());assert!(OriginalProcess::pin(pid).is_ok());
    let imported=File::from(c.rights[0].take().unwrap());
    assert_eq!(imported.metadata().unwrap().ino(),dummy.metadata().unwrap().ino());cloexec(imported.as_raw_fd()).unwrap();
    let (n,end)=recv(b.as_raw_fd(),&mut [0;1],until).unwrap();assert_eq!(n,0);assert!(end.empty());
}
#[test]
fn extra_truncated_packet_is_denied_and_still_owns_received_fds(){
    let (a,b)=local_pair();let dummy=public(b"not a vault",SEALS);send(&a,&dummy,&[3,4]);
    let (_,c)=recv(b.as_raw_fd(),&mut [0;1],Instant::now()+WAIT).unwrap();
    assert!(c.bad);assert!(!c.empty());assert!(c.rights[0].is_some());
}

#[cfg(target_arch="x86_64")]
#[test]
fn the_prepared_pair_reports_the_actual_original_pidfd_on_both_ends(){
    let original=super::super::kernel::OriginalProcess::current().unwrap();
    let (host,child)=super::super::kernel::fresh_pair().unwrap();
    let dummy=public(b"public local fixture",SEALS);
    for (from,to) in [(&host,&child),(&child,&host)]{
        send(from,&dummy,&[1u8;START_BYTES]);
        // SAFETY: half-close just this end of the fixture, once.
        assert_eq!(unsafe{libc::shutdown(from.as_raw_fd(),libc::SHUT_WR)},0);
        let until=Instant::now()+WAIT;
        let (n,mut packet)=recv(to.as_raw_fd(),&mut [0;START_BYTES],until).unwrap();
        assert_eq!(n,START_BYTES);assert!(!packet.bad);assert_eq!(packet.nrights,1);
        original.same_sender(File::from(packet.pidfd.take().unwrap())).unwrap();
        let (n,eof)=recv(to.as_raw_fd(),&mut [0;1],until).unwrap();
        assert_eq!(n,0);assert!(eof.empty());
    }
    assert_eq!(original.same_sender(dummy),Err(Fault::Handoff));
}
