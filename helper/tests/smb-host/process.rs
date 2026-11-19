use super::*;
use rustix::{net::{AddressFamily,SocketFlags,SocketType,socketpair},process::{pidfd_open,Pid,PidfdFlags}};
use std::{io::{Read,Write},os::{fd::OwnedFd,unix::net::UnixStream},process::{Command,Stdio},time::Duration};

#[test]
fn process_origin_does_not_accept_a_host_pid_nested_ns_or_duplicated_id(){
    assert_eq!(boot_pid(b"Pid:\t8412\nNSpid:\t8412\t1\n").unwrap(),8412);
    for b in [b"Pid:\t8412\nNSpid:\t8412\n".as_slice(),b"Pid:\t8412\nNSpid:\t8412\t7\n",
        b"Pid:\t8412\nNSpid:\t8412\t2\t1\n",b"Pid:\t-1\nNSpid:\t-1\n",b"Pid:\t3\nPid:\t3\nNSpid:\t3\t1\n"]{
        assert!(boot_pid(b).is_err());
    }
}

#[test]
fn every_capability_set_and_direct_parent_are_checked(){
    let clean="Tgid:\t42\nPid:\t42\nPPid:\t99\nNSpid:\t42\t1\nUid:\t0 0 0 0\nNoNewPrivs:\t1\n\
CapInh:\t0000000000000000\nCapEff:\t0000000000000000\nCapPrm:\t0000000000000000\nCapBnd:\t0000000000000000\nCapAmb:\t0000000000000000\n";
    status(clean.as_bytes(),42,99).unwrap();
    for key in ["CapEff","CapPrm","CapInh","CapBnd","CapAmb"]{
        let bad=clean.replace(&format!("{key}:\t0000000000000000"),&format!("{key}:\t0000000000200000"));
        assert!(status(bad.as_bytes(),42,99).is_err());
    }
    assert!(status(clean.as_bytes(),42,98).is_err());
    assert!(status(clean.replace("Privs:\t1","Privs:\t0").as_bytes(),42,99).is_err());
}

fn pair()->(File,File){
    let (a,b)=socketpair(AddressFamily::UNIX,SocketType::SEQPACKET,SocketFlags::CLOEXEC,None).unwrap();
    (File::from(a),File::from(b))
}
fn stream(f:OwnedFd)->UnixStream{
    let s=UnixStream::from(f);
    s.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(4))).unwrap();s
}

const CHILD:&str="HELPER_TEST_HELD_SMB_SOCKET_ENDPOINT";
#[test]
fn same_binary_payload(){
    if std::env::var_os(CHILD).as_deref()!=Some(std::ffi::OsStr::new("1")){return;}
    // Replicate Samba's duplicate-then-close, without Samba/systemd or any
    // user credentials. The kernel endpoint now lives at >=8, not fd 3.
    let mut socket=stream(rustix::io::fcntl_dupfd_cloexec(std::io::stdin(),8).unwrap());
    rustix::stdio::dup2_stdin(File::open("/dev/null").unwrap()).unwrap();
    socket.write_all(b"r").unwrap();let mut done=[0];socket.read_exact(&mut done).unwrap();assert_eq!(done,*b"q");
}

#[test]
fn real_child_holds_exact_renumbered_endpoint_but_not_the_storage_peer(){
    let (a,b)=pair();
    let mut child=Command::new(std::env::current_exe().unwrap())
        .args(["--exact","smb_host::process::tests::same_binary_payload"])
        .env(CHILD,"1").stdin(Stdio::from(a.try_clone().unwrap())).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let owner=File::from(pidfd_open(Pid::from_raw(child.id() as i32).unwrap(),PidfdFlags::empty()).unwrap());
    let mut remote=stream(b.try_clone().unwrap().into());
    let mut start=[0];remote.read_exact(&mut start).unwrap();assert_eq!(start,*b"r");
    waitable_live(&owner).unwrap();
    let p=File::from(open(format!("/proc/{}",child.id()).as_str(),DIR,Mode::empty()).unwrap());
    endpoint_set(&p,&a,&b).unwrap();
    assert!(endpoint_set(&p,&b,&a).is_err());
    let (wrong,other)=pair();assert!(endpoint_set(&p,&wrong,&other).is_err());
    remote.write_all(b"q").unwrap();assert!(child.wait().unwrap().success());
    assert!(waitable_live(&owner).is_err());
}

#[test]
fn release_size_above_64_mib_reaches_the_actual_bounded_reader(){
    use crate::fs_tools::hash_upto;
    // With no bytes available, UnexpectedEof proves the real read was attempted;
    // InvalidInput proves rejection before a read. No giant fixture is allocated.
    for n in [(64<<20)+1,128<<20] {
        let err=hash_upto(&mut io::empty(),n,super::super::MAX_IMAGE).unwrap_err();
        assert_eq!(err.kind(),io::ErrorKind::UnexpectedEof);
        assert_eq!(hash_upto(&mut io::empty(),n,64<<20).unwrap_err().kind(),io::ErrorKind::InvalidInput);
    }
    assert_eq!(hash_upto(&mut io::empty(),(128<<20)+1,super::super::MAX_IMAGE).unwrap_err().kind(),io::ErrorKind::InvalidInput);
}

#[test]
fn same_socket_inode_as_a_path_cannot_exchange_control_packets(){
    let (socket,_other)=pair();
    let process=File::from(open(format!("/proc/{}",std::process::id()).as_str(),DIR,Mode::empty()).unwrap());
    let dir=File::from(openat2(&process,c"fd",DIR,Mode::empty(),SAFE).unwrap());
    let real=CString::new(socket.as_raw_fd().to_string()).unwrap();
    let only=File::from(openat(&dir,real.as_c_str(),OFlags::PATH|OFlags::CLOEXEC,Mode::empty()).unwrap());
    let alias=CString::new(only.as_raw_fd().to_string()).unwrap();
    assert!(same(&only,&socket).unwrap());
    communicating(&process,&dir,real.as_c_str(),&socket).unwrap();
    assert!(communicating(&process,&dir,alias.as_c_str(),&socket).is_err());
    communicating(&process,&dir,real.as_c_str(),&socket).unwrap();
}
