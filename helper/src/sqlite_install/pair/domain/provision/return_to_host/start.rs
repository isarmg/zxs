//! A still-live approved parent must participate after the original child
//! has permanently dropped identity. A right-looking sealed request and
//! the static peer credential of a socketpair cannot substitute for this.
use super::super::super::handoff::start_control;
use crate::sqlite_install::bad;
use rustix::{fd::AsFd,net::{SendFlags,UCred,send,sockopt},process::{Gid,Pid,Uid},
    rand::{GetRandomFlags,getrandom}};
use std::{io,thread,time::{Duration,Instant}};

type Frame=[u8;65];
fn frame(kind:u8,nonce:&[u8;32],scope:&[u8;32])->Frame{
    let mut b=[0;65];b[0]=kind;b[1..33].copy_from_slice(nonce);b[33..].copy_from_slice(scope);b
}
fn nonce()->io::Result<[u8;32]>{
    let mut out=[0;32];loop{match getrandom(&mut out,GetRandomFlags::NONBLOCK){
        Err(rustix::io::Errno::INTR)=>continue,Err(e)=>return Err(e.into()),
        Ok(32) if out!=[0;32]=>return Ok(out),_=>return Err(bad("no complete fresh kernel challenge")),
    }}
}
fn put(fd:&impl AsFd,msg:&Frame)->io::Result<()>{
    match send(fd,msg,SendFlags::DONTWAIT|SendFlags::NOSIGNAL){
        Ok(65)=>Ok(()),Err(e)=>Err(e.into()),_=>Err(bad("incomplete dedicated permission record")),
    }
}
fn request(fd:&impl AsFd,cred:UCred,scope:&[u8;32])->io::Result<Option<[u8;32]>>{
    let b=match start_control(fd,cred){
        Err(e) if e.kind()==io::ErrorKind::WouldBlock=>return Ok(None),r=>r?,
    };
    let n:[u8;32]=b[1..33].try_into().expect("fixed challenge");
    if n==[0;32]||b!=frame(b'?',&n,scope){return Err(bad("worker requested a different original operation"));}
    Ok(Some(n))
}
/// Called ONLY by the parent that still owns this consumed foreground grant
/// and the unreaped real child. Ignore no received malformed/extra fields.
pub(in crate::sqlite_install::pair) fn respond(fd:&impl AsFd,cred:UCred,scope:&[u8;32],permit:impl FnOnce()->io::Result<()>)->io::Result<bool>{
    let Some(n)=request(fd,cred,scope)?else{return Ok(false)};
    permit()?;put(fd,&frame(b'!',&n,scope))?;Ok(true)
}
fn check_reply(fd:&impl AsFd,cred:UCred,n:&[u8;32],scope:&[u8;32])->io::Result<bool>{
    match start_control(fd,cred){
        Err(e) if e.kind()==io::ErrorKind::WouldBlock=>Ok(false),
        Ok(b) if b==frame(b'!',n,scope)=>Ok(true),
        Ok(_)=>Err(bad("different or replayed host start reply")),Err(e)=>Err(e),
    }
}
pub(in crate::sqlite_install::pair) fn child(fd:&impl AsFd,pid:Pid,scope:&[u8;32],alive:impl Fn()->io::Result<()>)->io::Result<()>{
    // Enable per-message attribution BEFORE the parent can know our nonce.
    sockopt::set_socket_passcred(fd,true)?;if !sockopt::socket_passcred(fd)?{return Err(bad("cannot attribute the live host reply"));}
    let start=Instant::now();let n=nonce()?;alive()?;put(fd,&frame(b'?',&n,scope))?;
    let parent=UCred{pid,uid:Uid::ROOT,gid:Gid::ROOT};
    loop{
        alive()?;if start.elapsed()>=Duration::from_secs(90){return Err(io::Error::new(io::ErrorKind::TimedOut,"no live foreground approval"));}
        if check_reply(fd,parent,&n,scope)?{return alive();}
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use rustix::net::{AddressFamily,SocketFlags,SocketType,socketpair};
    fn local()->UCred{UCred{pid:rustix::process::getpid(),uid:rustix::process::getuid(),gid:rustix::process::getgid()}}
    // Dedicated test-created sockets and dummy public bytes only. Never
    // select any host path, run the guest or require a privileged test.
    #[test]fn no_response_or_stale_challenge_is_not_authority_even_with_the_expected_peer(){
        let (p,c)=socketpair(AddressFamily::UNIX,SocketType::SEQPACKET,SocketFlags::CLOEXEC,None).unwrap();
        for f in [&p,&c]{sockopt::set_socket_passcred(f,true).unwrap();}
        let (n,s)=([2;32],[3;32]);
        put(&c,&frame(b'?',&n,&s)).unwrap();
        assert!(!check_reply(&c,local(),&n,&s).unwrap()); // sealed metadata alone cannot advance
        assert!(respond(&p,local(),&s,||Err(bad("no grant"))).is_err());
        assert!(!check_reply(&c,local(),&n,&s).unwrap());
        put(&p,&frame(b'!',&[4;32],&s)).unwrap();assert!(check_reply(&c,local(),&n,&s).is_err());
        put(&c,&frame(b'?',&n,&s)).unwrap();assert!(respond(&p,local(),&s,||Ok(())).unwrap());
        assert!(check_reply(&c,local(),&n,&s).unwrap());
        put(&c,&frame(b'?',&n,&s)).unwrap();let mut wrong=local();wrong.uid=Uid::from_raw(if wrong.uid.is_root(){1}else{0});
        assert!(respond(&p,wrong,&s,||panic!("wrong peer may not reach the grant")).is_err());
        put(&c,&frame(b'?',&n,&[8;32])).unwrap();
        assert!(respond(&p,local(),&s,||panic!("wrong scope may not reach the grant")).is_err());
        drop(p);assert!(check_reply(&c,local(),&n,&s).is_err());
    }
}
