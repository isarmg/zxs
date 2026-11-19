//! One bounded, nonsecret server challenge for one original SMB2 auth context.
//! No input from the client, password, response, user name or FILE claim is
//! accepted here. All outcomes leave authentication and SQL permissions unchanged.
use super::os;
use std::{io,os::fd::{AsRawFd,FromRawFd,OwnedFd,RawFd},time::{Duration,Instant}};

const LIMIT:usize=32;
const TTL:Duration=Duration::from_secs(30);
unsafe extern "C"{
    fn zxs_random(p:*mut u8,n:usize)->i32;
    fn zxs_smb_try_wait(ctl:i32,life:i32,peers:*const i32,n:usize,events:*mut u8)->i32;
    fn zxs_smb_try_accept(ctl:i32)->i32;
    fn zxs_smb_try_issue(fd:i32,challenge:*const u8)->i32;
    #[cfg(test)] fn zxs_smb_try_open(ctl:i32,born:*mut i64)->i32;
    #[cfg(test)] fn zxs_smb_try_read(fd:i32,born:i64,out:*mut u8)->i32;
    #[cfg(test)] fn zxs_smb_try_current(fd:i32,born:i64)->i32;
}
struct Attempt{peer:OwnedFd,issued:Instant,challenge:[u8;8]}
struct Active(Vec<Attempt>);
impl Active{
    fn new()->Self{Self(Vec::with_capacity(LIMIT))}
    fn admit(&mut self,peer:OwnedFd)->io::Result<()>{
        if self.0.len()>=LIMIT{return Err(io::ErrorKind::WouldBlock.into());}
        for _ in 0..4{
            let mut challenge=[0;8];
            os(unsafe{zxs_random(challenge.as_mut_ptr(),challenge.len())})?;
            if challenge==[0;8]||self.0.iter().any(|a|a.challenge==challenge){continue;}
            os(unsafe{zxs_smb_try_issue(peer.as_raw_fd(),challenge.as_ptr())})?;
            self.0.push(Attempt{peer,issued:Instant::now(),challenge});return Ok(());
        }
        Err(io::Error::other("no fresh server challenge"))
    }
    fn expire(&mut self,now:Instant,events:&[u8]){
        // events refer to the original poll order, not to vector positions
        // after compaction. Unsolicited input is *not read or logged*.
        let mut i=0;
        self.0.retain(|a|{
            let quiet=events[i]==0;i+=1;
            quiet && now.saturating_duration_since(a.issued)<TTL
        });
    }
}
fn accepted(ctl:RawFd)->io::Result<Option<OwnedFd>>{
    let fd=unsafe{zxs_smb_try_accept(ctl)};
    if fd== -2{return Ok(None);} // exact BEGIN consumed, no usable received fd
    if fd<0{return Err(io::Error::last_os_error());}
    // recvmsg checked framing and returned one original connected CLOEXEC
    // socket. From here there is exactly one owner of this received fd.
    Ok(Some(unsafe{OwnedFd::from_raw_fd(fd)}))
}
pub(super) fn serve(ctl:RawFd,life:&OwnedFd,stopping:impl Fn()->bool)->io::Result<()>{
    let mut active=Active::new();let mut draining=false;
    let revoke=|a:&mut Active|{
        let _=unsafe{super::zxs_smb_ctl_drain(ctl)};a.0.clear();
    };
    loop{
        if !draining && stopping(){revoke(&mut active);draining=true;}
        let mut fds=[-1;LIMIT];
        for (slot,a) in fds.iter_mut().zip(&active.0){*slot=a.peer.as_raw_fd();}
        let n=active.0.len();let mut events=[0u8;LIMIT+2];
        os(unsafe{zxs_smb_try_wait(if draining{-1}else{ctl},life.as_raw_fd(),fds.as_ptr(),n,events.as_mut_ptr())})?;
        if events[1]&2!=0{return Ok(());} // genuine lifetime pipe lost every writer
        if draining{continue;}
        if events[0]&2!=0 || stopping(){revoke(&mut active);draining=true;continue;}
        active.expire(Instant::now(),&events[2..2+n]);
        if events[0]&1!=0{
            match accepted(ctl){
                Ok(Some(peer))=>{
                    // Queue full, peer closed or randomness unavailable: just
                    // close that new endpoint. Never issue on the master.
                    let _=active.admit(peer);
                }
                Ok(None)=>(),
                Err(e) if matches!(e.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::Interrupted)=>(),
                Err(_)=>{revoke(&mut active);draining=true;}
            }
        }
    }
}
#[cfg(test)]
mod tests{
    use super::*;
    unsafe extern "C"{
        fn socketpair(domain:i32,kind:i32,protocol:i32,out:*mut i32)->i32;
        fn pipe2(out:*mut i32,flags:i32)->i32;
        fn geteuid()->u32;
        fn send(fd:i32,bytes:*const u8,n:usize,flags:i32)->isize;
    }
    fn pair()->(OwnedFd,OwnedFd){
        let mut f=[-1;2];os(unsafe{socketpair(1,5|0o2000000,0,f.as_mut_ptr())}).unwrap();
        unsafe{(OwnedFd::from_raw_fd(f[0]),OwnedFd::from_raw_fd(f[1]))}
    }
    fn pipe()->(OwnedFd,OwnedFd){
        let mut f=[-1;2];os(unsafe{pipe2(f.as_mut_ptr(),0o4000|0o2000000)}).unwrap();
        unsafe{(OwnedFd::from_raw_fd(f[0]),OwnedFd::from_raw_fd(f[1]))}
    }
    fn read(fd:&OwnedFd)->[u8;8]{
        let mut reply=[0;24];
        os(unsafe{super::super::zxs_smb_ctl_read(fd.as_raw_fd(),reply.as_mut_ptr(),reply.len())}).unwrap();
        assert_eq!(&reply[..16],b"ZXS-SAM/CHLNG-1\0");reply[16..].try_into().unwrap()
    }
    #[test]
    fn bounded_distinct_originals_and_no_authentication_messages(){
        let (ctl,_master)=pair();let(life,_keep)=pipe();let mut active=Active::new();let mut clients=Vec::new();
        for _ in 0..LIMIT{
            let(c,s)=pair();active.admit(s).unwrap();clients.push(c);
        }
        let values:Vec<_>=clients.iter().map(read).collect();assert!(values.iter().all(|a|*a!=[0;8]));
        for(i,a)in values.iter().enumerate(){assert!(!values[..i].contains(a));assert_eq!(*a,active.0[i].challenge);}
        let (extra,s)=pair();assert_eq!(active.admit(s).unwrap_err().kind(),io::ErrorKind::WouldBlock);
        assert!(unsafe{super::super::zxs_smb_ctl_live(extra.as_raw_fd())}<0);
        let rejected=1;assert_eq!(unsafe{send(clients[rejected].as_raw_fd(),b"x".as_ptr(),1,0)},1);
        let fds:Vec<_>=active.0.iter().map(|a|a.peer.as_raw_fd()).collect();let mut events=[0u8;LIMIT+2];
        os(unsafe{zxs_smb_try_wait(ctl.as_raw_fd(),life.as_raw_fd(),fds.as_ptr(),fds.len(),events.as_mut_ptr())}).unwrap();
        assert_ne!(events[2+rejected]&1,0);active.expire(Instant::now(),&events[2..]);
        assert_eq!(active.0.len(),LIMIT-1);assert!(!active.0.iter().any(|a|a.challenge==values[rejected]));
        assert!(unsafe{super::super::zxs_smb_ctl_live(clients[rejected].as_raw_fd())}<0);
        assert_eq!(unsafe{super::super::zxs_smb_ctl_live(clients[0].as_raw_fd())},0);
        let(c,s)=pair();active.admit(s).unwrap();assert_ne!(read(&c),[0;8]);
        let end=active.0.last().unwrap().issued+TTL;active.expire(end,&[0;LIMIT]);
        assert!(active.0.is_empty());
    }
    #[test]
    fn complete_begin_with_no_received_rights_rejects_only_that_attempt(){
        let(master,server)=pair();let (old,peer)=pair();let mut active=Active::new();
        active.admit(peer).unwrap();let original=read(&old);
        // Same observable packet as a kernel-discarded local right. This is
        // not described as an injected system-wide RLIMIT/ENOMEM failure.
        let tag=b"ZXS-SAM/BEGIN-1\0";
        os(unsafe{super::super::zxs_smb_ctl_write(master.as_raw_fd(),tag.as_ptr(),tag.len())}).unwrap();
        assert!(accepted(server.as_raw_fd()).unwrap().is_none());
        assert_eq!(active.0[0].challenge,original);
        assert_eq!(unsafe{super::super::zxs_smb_ctl_live(old.as_raw_fd())},0);
        let bad=b"ZXS-SAM/OTHER-1\0";
        os(unsafe{super::super::zxs_smb_ctl_write(master.as_raw_fd(),bad.as_ptr(),bad.len())}).unwrap();
        assert!(accepted(server.as_raw_fd()).is_err()); // malformed master frame, not a local shortage
    }
    #[test]
    #[ignore="source-only: actual root-created private endpoints; not a login or a network client"]
    fn independent_registrations_need_no_common_reply_or_global_wait(){
        assert_eq!(unsafe{geteuid()},0,"requires the real root socket-creator fixture");
        let(master,server)=pair();let open=||{
            let mut started=-1;let v=unsafe{zxs_smb_try_open(master.as_raw_fd(),&mut started)};
            assert!(v>=0);(unsafe{OwnedFd::from_raw_fd(v)},started)
        };
        let(a,ta)=open();let(b,tb)=open();let sa=accepted(server.as_raw_fd()).unwrap().unwrap();
        let sb=accepted(server.as_raw_fd()).unwrap().unwrap();let mut seen=[0;8];
        assert!(unsafe{zxs_smb_try_read(a.as_raw_fd(),ta,seen.as_mut_ptr())}<0);
        assert_eq!(io::Error::last_os_error().kind(),io::ErrorKind::WouldBlock);
        let first=[1;8];let second=[2;8];
        os(unsafe{zxs_smb_try_issue(sb.as_raw_fd(),second.as_ptr())}).unwrap();
        os(unsafe{zxs_smb_try_read(b.as_raw_fd(),tb,seen.as_mut_ptr())}).unwrap();assert_eq!(seen,second);
        // Worker kept this fd open; the native monotonic bound still refuses.
        assert!(unsafe{zxs_smb_try_current(b.as_raw_fd(),tb-30_000)}<0);
        assert_eq!(unsafe{super::super::zxs_smb_ctl_live(b.as_raw_fd())},0);
        os(unsafe{zxs_smb_try_issue(sa.as_raw_fd(),first.as_ptr())}).unwrap();
        os(unsafe{zxs_smb_try_read(a.as_raw_fd(),ta,seen.as_mut_ptr())}).unwrap();assert_eq!(seen,first);
        drop(sa);assert!(unsafe{zxs_smb_try_current(a.as_raw_fd(),ta)}<0);
        assert_eq!(unsafe{zxs_smb_try_current(b.as_raw_fd(),tb)},0);
    }
}
