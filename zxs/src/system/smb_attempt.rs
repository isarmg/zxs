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
    fn zxs_smb_origin_recv(ctl:i32,tcp:*mut u64,sender:*mut i32,object:*mut i32)->i32;
    fn zxs_smb_process_live(fd:i32)->i32;
    fn zxs_smb_same_process(a:i32,b:i32)->i32;
    fn zxs_smb_attempt_peer(fd:i32,sender:i32)->i32;
    fn zxs_smb_try_issue(fd:i32,challenge:*const u8)->i32;
    #[cfg(test)] fn zxs_smb_try_open(ctl:i32,net:u64,sk:u64,born:*mut i64)->i32;
    #[cfg(test)] fn zxs_smb_try_read(fd:i32,born:i64,out:*mut u8)->i32;
    #[cfg(test)] fn zxs_smb_try_current(fd:i32,born:i64)->i32;
}
struct Registered{peer:OwnedFd,worker:OwnedFd,tcp:[u64;2]}
struct Attempt{origin:Registered,issued:Instant,challenge:[u8;8]}
struct Active(Vec<Attempt>);
impl Active{
    fn new()->Self{Self(Vec::with_capacity(LIMIT))}
    fn admit(&mut self,origin:Registered)->io::Result<()>{
        if origin.tcp.contains(&0)||unsafe{zxs_smb_process_live(origin.worker.as_raw_fd())}!=0{
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        if self.0.len()>=LIMIT{return Err(io::ErrorKind::WouldBlock.into());}
        for _ in 0..4{
            let mut challenge=[0;8];
            os(unsafe{zxs_random(challenge.as_mut_ptr(),challenge.len())})?;
            if challenge==[0;8]||self.0.iter().any(|a|a.challenge==challenge){continue;}
            os(unsafe{zxs_smb_try_issue(origin.peer.as_raw_fd(),challenge.as_ptr())})?;
            self.0.push(Attempt{origin,issued:Instant::now(),challenge});return Ok(());
        }
        Err(io::Error::other("no fresh server challenge"))
    }
    fn expire(&mut self,now:Instant,events:&[u8]){
        // events refer to the original poll order, not to vector positions
        // after compaction. Unsolicited input is *not read or logged*.
        let mut i=0;
        self.0.retain(|a|{
            let quiet=events[i]==0;i+=1;
            quiet && now.saturating_duration_since(a.issued)<TTL && unsafe{zxs_smb_process_live(a.origin.worker.as_raw_fd())==0}
        });
    }
}
#[derive(Clone,Copy,PartialEq,Eq)]
enum Role { Dispatch, Begin }
struct Delivery { role:Role,tcp:[u64;2],sender:OwnedFd,object:OwnedFd }
fn accepted(ctl:RawFd)->io::Result<Option<Delivery>>{
    let(mut tcp,mut sender,mut object)=([0;2],-1,-1);
    let role=match unsafe{zxs_smb_origin_recv(ctl,tcp.as_mut_ptr(),&mut sender,&mut object)}{
        1=>Role::Dispatch,2=>Role::Begin,-2=>return Ok(None),_=>return Err(io::Error::last_os_error()),
    };
    // The kernel sender pidfd and the one offered right are independent
    // CLOEXEC descriptors. The C side closes every extra/invalid received FD.
    Ok(Some(Delivery{role,tcp,sender:unsafe{OwnedFd::from_raw_fd(sender)},object:unsafe{OwnedFd::from_raw_fd(object)}}))
}
const WORKER_LIMIT:usize=256;
struct Worker{pid:OwnedFd,tcp:[u64;2]}
struct Workers<'a>{master:&'a OwnedFd,live:Vec<Worker>}
fn same(a:&OwnedFd,b:&OwnedFd)->bool{unsafe{zxs_smb_same_process(a.as_raw_fd(),b.as_raw_fd())==1}}
impl<'a> Workers<'a>{
    fn new(master:&'a OwnedFd)->Self{Self{master,live:Vec::with_capacity(WORKER_LIMIT)}}
    fn sweep(&mut self){self.live.retain(|w|unsafe{zxs_smb_process_live(w.pid.as_raw_fd())==0});}
    fn receive(&mut self,d:Delivery)->Option<Registered>{
        if unsafe{zxs_smb_process_live(self.master.as_raw_fd())}!=0{return None;}
        match d.role{
            Role::Dispatch=>{
                if !same(self.master,&d.sender){return None;}
                if self.live.len()==WORKER_LIMIT{self.sweep();}
                if same(&d.object,self.master) ||
                    unsafe{zxs_smb_process_live(d.object.as_raw_fd())}!=0 ||
                    self.live.len()==WORKER_LIMIT || self.live.iter().any(|w|w.tcp==d.tcp||same(&w.pid,&d.object)){
                    return None;
                }
                self.live.push(Worker{pid:d.object,tcp:d.tcp});None
            }
            Role::Begin=>{
                if !self.live.iter().any(|w|w.tcp==d.tcp && same(&w.pid,&d.sender)) ||
                    unsafe{zxs_smb_attempt_peer(d.object.as_raw_fd(),d.sender.as_raw_fd())}!=0{return None;}
                Some(Registered{peer:d.object,worker:d.sender,tcp:d.tcp})
            }
        }
    }
}
pub(super) fn serve(ctl:RawFd,life:&OwnedFd,master:&OwnedFd,stopping:impl Fn()->bool)->io::Result<()>{
    let mut active=Active::new();let mut workers=Workers::new(master);let mut swept=Instant::now();let mut draining=false;
    let revoke=|a:&mut Active|{
        let _=unsafe{super::zxs_smb_ctl_drain(ctl)};a.0.clear();
    };
    loop{
        if !draining && swept.elapsed()>=Duration::from_secs(1){workers.sweep();swept=Instant::now();}
        if !draining && (stopping()||unsafe{zxs_smb_process_live(master.as_raw_fd())}!=0){revoke(&mut active);workers.live.clear();draining=true;}
        let mut fds=[-1;LIMIT];
        for (slot,a) in fds.iter_mut().zip(&active.0){*slot=a.origin.peer.as_raw_fd();}
        let n=active.0.len();let mut events=[0u8;LIMIT+2];
        os(unsafe{zxs_smb_try_wait(if draining{-1}else{ctl},life.as_raw_fd(),fds.as_ptr(),n,events.as_mut_ptr())})?;
        if events[1]&2!=0{return Ok(());} // genuine lifetime pipe lost every writer
        if draining{continue;}
        if events[0]&2!=0 || stopping(){revoke(&mut active);draining=true;continue;}
        active.expire(Instant::now(),&events[2..2+n]);
        if events[0]&1!=0{
            match accepted(ctl){
                Ok(Some(d))=>{
                    // Queue full, peer closed or randomness unavailable: just
                    // close that new endpoint. Never issue on the master.
                    if let Some(peer)=workers.receive(d){let _=active.admit(peer);}
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
    fn scope(peer:OwnedFd)->Registered{
        unsafe extern "C"{fn getsockopt(fd:i32,level:i32,op:i32,data:*mut i32,len:*mut u32)->i32;}
        let(mut p,mut n)=(-1,4u32);
        os(unsafe{getsockopt(peer.as_raw_fd(),1,77,&mut p,&mut n)}).unwrap();assert!(p>=0 && n==4);
        Registered{peer,worker:unsafe{OwnedFd::from_raw_fd(p)},tcp:[1,2]}
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
            let(c,s)=pair();active.admit(scope(s)).unwrap();clients.push(c);
        }
        let values:Vec<_>=clients.iter().map(read).collect();assert!(values.iter().all(|a|*a!=[0;8]));
        for(i,a)in values.iter().enumerate(){assert!(!values[..i].contains(a));assert_eq!(*a,active.0[i].challenge);}
        let (extra,s)=pair();assert_eq!(active.admit(scope(s)).unwrap_err().kind(),io::ErrorKind::WouldBlock);
        assert!(unsafe{super::super::zxs_smb_ctl_live(extra.as_raw_fd())}<0);
        let rejected=1;assert_eq!(unsafe{send(clients[rejected].as_raw_fd(),b"x".as_ptr(),1,0)},1);
        let fds:Vec<_>=active.0.iter().map(|a|a.origin.peer.as_raw_fd()).collect();let mut events=[0u8;LIMIT+2];
        os(unsafe{zxs_smb_try_wait(ctl.as_raw_fd(),life.as_raw_fd(),fds.as_ptr(),fds.len(),events.as_mut_ptr())}).unwrap();
        assert_ne!(events[2+rejected]&1,0);active.expire(Instant::now(),&events[2..]);
        assert_eq!(active.0.len(),LIMIT-1);assert!(!active.0.iter().any(|a|a.challenge==values[rejected]));
        assert!(unsafe{super::super::zxs_smb_ctl_live(clients[rejected].as_raw_fd())}<0);
        assert_eq!(unsafe{super::super::zxs_smb_ctl_live(clients[0].as_raw_fd())},0);
        let(c,s)=pair();active.admit(scope(s)).unwrap();assert_ne!(read(&c),[0;8]);
        let end=active.0.last().unwrap().issued+TTL;active.expire(end,&[0;LIMIT]);
        assert!(active.0.is_empty());
    }
    #[test]
    fn complete_begin_with_no_received_rights_rejects_only_that_attempt(){
        let(master,server)=pair();let (old,peer)=pair();let mut active=Active::new();
        active.admit(scope(peer)).unwrap();let original=read(&old);
        // Same observable packet as a kernel-discarded local right. This is
        // not described as an injected system-wide RLIMIT/ENOMEM failure.
        let mut tag=[0u8;32];tag[..16].copy_from_slice(b"ZXS-SAM/BEGIN-2\0");
        os(unsafe{super::super::zxs_smb_ctl_write(master.as_raw_fd(),tag.as_ptr(),tag.len())}).unwrap();
        assert!(accepted(server.as_raw_fd()).unwrap().is_none());
        assert_eq!(active.0[0].challenge,original);
        assert_eq!(unsafe{super::super::zxs_smb_ctl_live(old.as_raw_fd())},0);
        let bad=b"ZXS-SAM/OTHER-1\0";
        os(unsafe{super::super::zxs_smb_ctl_write(master.as_raw_fd(),bad.as_ptr(),bad.len())}).unwrap();
        assert!(accepted(server.as_raw_fd()).is_err()); // malformed master frame, not a local shortage
    }
    #[test]
    #[ignore="source-only: dedicated root without CAP_SYS_ADMIN; real pidfds and fork, not a login"]
    fn dispatched_sender_and_receiving_endpoint_must_be_the_same_original_worker(){
        unsafe extern "C"{
            fn fork()->i32;fn close(fd:i32)->i32;fn _exit(rc:i32)->!;
            #[link_name="read"] fn one_read(fd:i32,p:*mut u8,n:usize)->isize;
            #[link_name="write"] fn one_write(fd:i32,p:*const u8,n:usize)->isize;
            fn zxs_smb_sender_isolated()->i32;
            fn zxs_smb_dispatch(fd:i32,child:i32,net:u64,sk:u64)->i32;
            fn waitpid(pid:i32,status:*mut i32,flags:i32)->i32;
        }
        fn blocking_pipe()->(OwnedFd,OwnedFd){
            let mut f=[-1;2];os(unsafe{pipe2(f.as_mut_ptr(),0o2000000)}).unwrap();
            unsafe{(OwnedFd::from_raw_fd(f[0]),OwnedFd::from_raw_fd(f[1]))}
        }
        assert_eq!(unsafe{geteuid()},0);os(unsafe{zxs_smb_sender_isolated()}).unwrap();
        let(ctl,storage)=pair();let(seed,_other)=pair();let master=scope(seed).worker;
        let mut w=Workers::new(&master);let(go,give)=blocking_pipe();let(done,tell)=blocking_pipe();let(stay,end)=blocking_pipe();
        let(fd,g,d,k)=(ctl.as_raw_fd(),go.as_raw_fd(),tell.as_raw_fd(),stay.as_raw_fd());
        let child=unsafe{fork()};assert!(child>=0);
        if child==0{unsafe{
            // Do not allocate/use the Rust harness after fork. No second
            // process retains a write end that could keep itself alive.
            close(give.as_raw_fd());close(end.as_raw_fd());close(done.as_raw_fd());close(storage.as_raw_fd());
            let mut b=0;let mut born=0;
            if one_read(g,&mut b,1)!=1{_exit(2);}
            let attempt=zxs_smb_try_open(fd,17,23,&mut born);
            if attempt<0 || one_write(d,&1,1)!=1{_exit(3);}
            let _=one_read(k,&mut b,1);close(attempt);_exit(0)
        }}
        drop(go);drop(tell);drop(stay);
        // Simulate scheduler ordering: all legal senders supplied their
        // own credentials. Dispatch may already be queued before the flag.
        os(unsafe{zxs_smb_dispatch(fd,child,17,23)}).unwrap();
        os(unsafe{super::super::zxs_smb_origin_enable(storage.as_raw_fd())}).unwrap();
        let msg=||accepted(storage.as_raw_fd()).unwrap().unwrap();
        let d=msg();assert!(d.role==Role::Dispatch && same(&d.sender,&master));
        assert!(w.receive(d).is_none());assert_eq!(w.live.len(),1);
        // Master has fd3, knows the pair, and can submit BEGIN but is not
        // this worker. It receives nothing from the protected attempt.
        let mut born=0;let other=unsafe{zxs_smb_try_open(fd,17,23,&mut born)};assert!(other>=0);
        let other=unsafe{OwnedFd::from_raw_fd(other)};assert!(w.receive(msg()).is_none());
        assert!(unsafe{super::super::zxs_smb_ctl_live(other.as_raw_fd())}<0);
        assert_eq!(unsafe{one_write(give.as_raw_fd(),&1,1)},1);drop(give);
        let mut b=0;assert_eq!(unsafe{one_read(done.as_raw_fd(),&mut b,1)},1);
        let d=msg();assert!(d.role==Role::Begin && d.tcp==[17,23] && !same(&d.sender,&master));
        let(fake,_)=pair();assert!(unsafe{zxs_smb_attempt_peer(fake.as_raw_fd(),d.sender.as_raw_fd())}<0);
        let bad=Delivery{tcp:[17,24],role:Role::Begin,sender:d.sender.try_clone().unwrap(),object:d.object.try_clone().unwrap()};
        assert!(w.receive(bad).is_none());
        let good=w.receive(d).expect("registered worker, exact transport, original newly created endpoint");
        assert!(same(&good.worker,&w.live[0].pid));assert_eq!(good.tcp,[17,23]);
        drop(end);let mut status=-1;assert_eq!(unsafe{waitpid(child,&mut status,0)},child);assert_eq!(status,0);
        w.sweep();assert!(w.live.is_empty());assert!(unsafe{zxs_smb_process_live(good.worker.as_raw_fd())}<0);
    }
    #[test]
    #[ignore="source-only: actual root-created private endpoints; not a login or a network client"]
    fn independent_registrations_need_no_common_reply_or_global_wait(){
        assert_eq!(unsafe{geteuid()},0,"requires the real root socket-creator fixture");
        let(master,server)=pair();os(unsafe{super::super::zxs_smb_origin_enable(server.as_raw_fd())}).unwrap();let open=||{
            let mut started=-1;let v=unsafe{zxs_smb_try_open(master.as_raw_fd(),1,2,&mut started)};
            assert!(v>=0);(unsafe{OwnedFd::from_raw_fd(v)},started)
        };
        let(a,ta)=open();let(b,tb)=open();let da=accepted(server.as_raw_fd()).unwrap().unwrap();
        let db=accepted(server.as_raw_fd()).unwrap().unwrap();
        assert!(da.role==Role::Begin && db.role==Role::Begin && da.tcp==[1,2]&&db.tcp==da.tcp);
        assert!(same(&da.sender,&db.sender));
        os(unsafe{zxs_smb_attempt_peer(da.object.as_raw_fd(),da.sender.as_raw_fd())}).unwrap();
        let(sa,sb)=(da.object,db.object);let mut seen=[0;8];
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
