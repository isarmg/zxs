//! One bounded, nonsecret server challenge for one original SMB2 auth context.
//! A one-time nonsecret generation cut is also available to that same attempt.
//! No network input, password, response, user name or FILE claim is
//! accepted here. The original worker may promote only its own consumed cut,
//! after Samba's final successful local update. Share access is a separate cut.
use super::{Slot,os};
#[cfg(test)] use super::super::{InitialReader,SmbSession,denied,share_mode::Setting};
use crate::identity::ShareId;
use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
use std::{io,os::fd::{AsRawFd,FromRawFd,OwnedFd,RawFd},time::{Duration,Instant}};

const LIMIT:usize=32;
const SESSIONS:usize=64;
const TTL:Duration=Duration::from_secs(30);
unsafe extern "C"{
    fn zxs_random(p:*mut u8,n:usize)->i32;
    fn zxs_smb_try_wait(ctl:i32,life:i32,peers:*const i32,n:usize,events:*mut u8)->i32;
    fn zxs_smb_origin_recv(ctl:i32,tcp:*mut u64,sender:*mut i32,object:*mut i32)->i32;
    fn zxs_smb_process_live(fd:i32)->i32;
    fn zxs_smb_same_process(a:i32,b:i32)->i32;
    fn zxs_smb_attempt_peer(fd:i32,sender:i32)->i32;
    fn zxs_smb_active_take(fd:i32,challenge:*const u8)->i32;
    fn zxs_smb_tree_take(fd:i32,worker:i32,share:*mut u8)->i32;

    fn zxs_smb_cut_take(fd:i32,challenge:*const u8)->i32;
    fn zxs_smb_cut_reply(fd:i32,challenge:*const u8,identity:*const u8)->i32;
    fn zxs_smb_try_issue(fd:i32,challenge:*const u8)->i32;
    #[cfg(test)] fn zxs_smb_try_open(ctl:i32,net:u64,sk:u64,born:*mut i64)->i32;
    #[cfg(test)] fn zxs_smb_try_read(fd:i32,born:i64,out:*mut u8)->i32;
    #[cfg(test)] fn zxs_smb_try_current(fd:i32,born:i64)->i32;
}
struct Registered{peer:OwnedFd,worker:OwnedFd,tcp:[u64;2]}
struct Attempt{origin:Registered,slot:Slot,issued:Instant,challenge:[u8;8],cut_sent:bool}
struct Active(Vec<Attempt>);
impl Active{
    fn new()->Self{Self(Vec::with_capacity(LIMIT))}
    fn admit(&mut self,origin:Registered,select:impl FnOnce()->io::Result<Slot>)->io::Result<()>{
        if origin.tcp.contains(&0)||unsafe{zxs_smb_process_live(origin.worker.as_raw_fd())}!=0{
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        if self.0.len()>=LIMIT{return Err(io::ErrorKind::WouldBlock.into());}
        // Do not let an unregistered sender or a full queue trigger a DB read.
        // The original S read transaction is already closed before sending.
        let slot=select()?;
        self.0.retain(|a|a.slot==slot);
        for _ in 0..4{
            let mut challenge=[0;8];
            os(unsafe{zxs_random(challenge.as_mut_ptr(),challenge.len())})?;
            if challenge==[0;8]||self.0.iter().any(|a|a.challenge==challenge){continue;}
            os(unsafe{zxs_smb_try_issue(origin.peer.as_raw_fd(),challenge.as_ptr())})?;
            self.0.push(Attempt{origin,slot,issued:Instant::now(),challenge,cut_sent:false});return Ok(());
        }
        Err(io::Error::other("no fresh server challenge"))
    }
    fn recheck(&mut self,select:impl FnOnce()->io::Result<Slot>){
        if self.0.is_empty(){return;}
        // One fresh short cut for all attempts. No periodic query when idle.
        match select(){Ok(slot)=>self.0.retain(|a|a.slot==slot),Err(_)=>self.0.clear()}
    }
    fn observe_with(&mut self,now:Instant,events:&[u8],select:impl Fn()->io::Result<Slot>,live:&mut Sessions){
        let mut i=0;let mut current:Option<Option<Slot>>=None;
        for &e in events{
            let a=&mut self.0[i];let fd=a.origin.peer.as_raw_fd();
            let mut promote=false;
            let keep=if e&2!=0||now.saturating_duration_since(a.issued)>=TTL||
                unsafe{zxs_smb_process_live(a.origin.worker.as_raw_fd())}!=0{false}
            else if e==0{true}
            else if e!=1{false}
            else if a.cut_sent{
                // Native sent this on precisely this endpoint, after committing
                // the NEW local Samba session. A second generation request,
                // another attempt's challenge, expiry, or full budget denies.
                promote=live.0.len()<SESSIONS && unsafe{zxs_smb_active_take(fd,a.challenge.as_ptr())}==0 &&
                    current.get_or_insert_with(||select().ok()).as_ref()==Some(&a.slot) && a.issued.elapsed()<TTL;
                false
            }else if unsafe{zxs_smb_cut_take(fd,a.challenge.as_ptr())}!=0 ||
                current.get_or_insert_with(||select().ok()).as_ref()!=Some(&a.slot){false}
            else if let Some(bytes)=generation_bytes(&a.slot){
                a.cut_sent=true;
                a.issued.elapsed()<TTL && unsafe{zxs_smb_cut_reply(fd,a.challenge.as_ptr(),bytes.as_ptr())==0}
            }else{false};
            if keep{i+=1;}else{
                let a=self.0.remove(i);
                if promote{live.0.push(NativeSession{peer:a.origin,slot:a.slot,alive:Arc::new(AtomicBool::new(true))});}
            }
        }
    }
    #[cfg(test)]
    fn observe(&mut self,now:Instant,events:&[u8],select:impl Fn()->io::Result<Slot>){
        self.observe_with(now,events,select,&mut Sessions::new());
    }
    #[cfg(test)]
    fn expire(&mut self,now:Instant,events:&[u8]){self.observe(now,events,||Err(io::ErrorKind::PermissionDenied.into()));}

}
/// Exists only while the private endpoint of this one native session survives.
/// Two sessions on the same worker/TCP do not share an endpoint or identity.
pub(super) struct NativeSession{peer:Registered,slot:Slot,alive:Arc<AtomicBool>}
impl Drop for NativeSession{fn drop(&mut self){self.alive.store(false,Ordering::Release);}}
pub(super) struct TreeOrigin{
    pub(super) slot:Slot,pub(super) alive:Arc<AtomicBool>,
    pub(super) session:OwnedFd,pub(super) worker:OwnedFd,
}
impl NativeSession{
    fn claim(&self)->io::Result<TreeOrigin>{
        Ok(TreeOrigin{slot:self.slot.clone(),alive:self.alive.clone(),session:self.peer.peer.try_clone()?,
            worker:self.peer.worker.try_clone()?})
    }
    #[cfg(test)]
    pub(super) fn select(&self,src:&InitialReader,share:ShareId)->io::Result<Setting>{
        if src.pool!=self.slot.pool{return Err(denied());}
        let s=SmbSession{source:src,original:self.slot.original.clone()};
        let t=s.connect_id(share)?;Ok(t.setting)
    }
}
struct Sessions(Vec<NativeSession>);
impl Sessions{
    fn new()->Self{Self(Vec::with_capacity(SESSIONS))}
    fn observe(&mut self,events:&[u8],mut open:impl FnMut(TreeOrigin,ShareId,OwnedFd)){
        let mut i=0;
        self.0.retain(|session|{
            let e=events[i];i+=1;
            if e&2!=0||unsafe{zxs_smb_process_live(session.peer.worker.as_raw_fd())}!=0{return false;}
            if e==1{
                let mut id=[0;16];
                let n=unsafe{zxs_smb_tree_take(session.peer.peer.as_raw_fd(),session.peer.worker.as_raw_fd(),id.as_mut_ptr())};
                // Bad/truncated/closed fresh return fd: do not disclose or
                // cancel unrelated trees/SMB sessions. No answer on the master.
                if n>=0{
                    let reply=unsafe{OwnedFd::from_raw_fd(n)};let share=ShareId(id);
                    // Admission/slow real-F work belongs to a bounded worker.
                    // Never block dispatch of another session on a volume.
                    if let Ok(origin)=session.claim(){open(origin,share,reply);}
                }
            }
            true
        });
    }
}
/// One fixed nonsecret tuple; the slot was selected by the storage process,
/// not from the SMB client or request. Layout is independent of native padding.
pub(super) fn generation_bytes(s:&Slot)->Option<[u8;149]>{
    let c=&s.original;
    if c.generation<=0 || crate::password::login_name(&c.login).is_err(){return None;}
    let mut out=[0;149];out[..16].copy_from_slice(&s.pool.0);
    out[16..32].copy_from_slice(&c.account.0);out[32..48].copy_from_slice(&c.op.0);
    out[48..56].copy_from_slice(&c.generation.to_be_bytes());out[56..84].copy_from_slice(&c.sid);
    out[84]=u8::try_from(c.login.len()).ok()?;out[85..85+c.login.len()].copy_from_slice(&c.login);Some(out)
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
pub(super) fn serve(ctl:RawFd,life:&OwnedFd,master:&OwnedFd,stopping:impl Fn()->bool,select:impl Fn()->io::Result<Slot>,mut open:impl FnMut(TreeOrigin,ShareId,OwnedFd))->io::Result<()>{
    let mut active=Active::new();let mut sessions=Sessions::new();let mut workers=Workers::new(master);let mut swept=Instant::now();let mut draining=false;
    let revoke=|a:&mut Active,s:&mut Sessions|{
        let _=unsafe{super::zxs_smb_ctl_drain(ctl)};a.0.clear();s.0.clear();
    };
    loop{
        if !draining && swept.elapsed()>=Duration::from_secs(1){workers.sweep();active.recheck(&select);swept=Instant::now();}
        if !draining && (stopping()||unsafe{zxs_smb_process_live(master.as_raw_fd())}!=0){revoke(&mut active,&mut sessions);workers.live.clear();draining=true;}
        let mut fds=[-1;LIMIT+SESSIONS];let na=active.0.len();let n=na+sessions.0.len();
        for (slot,a) in fds[..na].iter_mut().zip(&active.0){*slot=a.origin.peer.as_raw_fd();}
        for (slot,s) in fds[na..n].iter_mut().zip(&sessions.0){*slot=s.peer.peer.as_raw_fd();}
        let mut events=[0u8;LIMIT+SESSIONS+2];
        os(unsafe{zxs_smb_try_wait(if draining{-1}else{ctl},life.as_raw_fd(),fds.as_ptr(),n,events.as_mut_ptr())})?;
        if events[1]&2!=0{return Ok(());} // genuine lifetime pipe lost every writer
        if draining{continue;}
        if events[0]&2!=0 || stopping() || unsafe{zxs_smb_process_live(master.as_raw_fd())}!=0{revoke(&mut active,&mut sessions);draining=true;continue;}
        // Only the original poll participants consume these result slots.
        sessions.observe(&events[2+na..2+n],&mut open);
        active.observe_with(Instant::now(),&events[2..2+na],&select,&mut sessions);
        if events[0]&1!=0{
            match accepted(ctl){
                Ok(Some(d))=>{
                    // Queue full, peer closed or randomness unavailable: just
                    // close that new endpoint. Never issue on the master.
                    if let Some(peer)=workers.receive(d){let _=active.admit(peer,&select);}
                }
                Ok(None)=>(),
                Err(e) if matches!(e.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::Interrupted)=>(),
                Err(_)=>{revoke(&mut active,&mut sessions);draining=true;}
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
    fn synthetic()->io::Result<Slot>{
        Ok(Slot{pool:crate::PoolId([4;16]),original:super::super::Credential{
            account:crate::identity::AccountId([9;16]),instance:[7;16],op:crate::OperationId([8;16]),generation:1,
            login:b"only.smb".to_vec(),sid:[7;28]}})
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
            let(c,s)=pair();active.admit(scope(s),synthetic).unwrap();clients.push(c);
        }
        let values:Vec<_>=clients.iter().map(read).collect();assert!(values.iter().all(|a|*a!=[0;8]));
        for(i,a)in values.iter().enumerate(){assert!(!values[..i].contains(a));assert_eq!(*a,active.0[i].challenge);}
        let (extra,s)=pair();assert_eq!(active.admit(scope(s),synthetic).unwrap_err().kind(),io::ErrorKind::WouldBlock);
        assert!(unsafe{super::super::zxs_smb_ctl_live(extra.as_raw_fd())}<0);
        let rejected=1;assert_eq!(unsafe{send(clients[rejected].as_raw_fd(),b"x".as_ptr(),1,0)},1);
        let fds:Vec<_>=active.0.iter().map(|a|a.origin.peer.as_raw_fd()).collect();let mut events=[0u8;LIMIT+2];
        os(unsafe{zxs_smb_try_wait(ctl.as_raw_fd(),life.as_raw_fd(),fds.as_ptr(),fds.len(),events.as_mut_ptr())}).unwrap();
        assert_ne!(events[2+rejected]&1,0);active.expire(Instant::now(),&events[2..]);
        assert_eq!(active.0.len(),LIMIT-1);assert!(!active.0.iter().any(|a|a.challenge==values[rejected]));
        assert!(unsafe{super::super::zxs_smb_ctl_live(clients[rejected].as_raw_fd())}<0);
        assert_eq!(unsafe{super::super::zxs_smb_ctl_live(clients[0].as_raw_fd())},0);
        let(c,s)=pair();active.admit(scope(s),synthetic).unwrap();assert_ne!(read(&c),[0;8]);
        let end=active.0.last().unwrap().issued+TTL;active.expire(end,&[0;LIMIT]);
        assert!(active.0.is_empty());
    }
    #[test]
    #[ignore="source-only: synthetic native completion event and real non-root SQLite; not SMB authentication"]
    fn one_original_post_update_peer_can_select_only_current_published_business_shares(){
        use crate::{OperationId,VolumeId,system::reader::{InitialReader,web_pool},test_support::PrivateTemp};
        use super::super::super::share_mode::Mode;
        unsafe extern "C"{fn zxs_smb_active_send(fd:i32,ch:*const u8)->i32;}
        let tmp=PrivateTemp::under(&std::env::temp_dir());let (path,p)=web_pool(&tmp);
        let src=InitialReader::open(&path,p).unwrap();let peer=InitialReader::open(&path,p).unwrap();
        let web=peer.authenticate_web("first.admin","correct horse battery staple").unwrap();
        let share=web.publish_root(OperationId([0x98;16]),VolumeId([0x13;16]),"Root").unwrap();
        let select=||src.at(|db,c|Ok(Slot{pool:p,original:super::super::super::Credential::current(db,&c)?}));
        let mut active=Active::new();let mut live=Sessions::new();
        let(c,s)=pair();active.admit(scope(s),select).unwrap();let ch=read(&c);
        // Fixture sends a native message before the late cut. The ordinary
        // challenge alone cannot construct the production business session.
        os(unsafe{zxs_smb_active_send(c.as_raw_fd(),ch.as_ptr())}).unwrap();
        active.observe_with(Instant::now(),&[1],select,&mut live);
        assert!(live.0.is_empty()&&active.0.is_empty());drop(c);
        let(c,s)=pair();active.admit(scope(s),select).unwrap();let ch=read(&c);
        let mut msg=[0;24];msg[..15].copy_from_slice(b"ZXS-SAM/GNCUT-1");msg[16..].copy_from_slice(&ch);
        os(unsafe{super::super::zxs_smb_ctl_write(c.as_raw_fd(),msg.as_ptr(),24)}).unwrap();
        active.observe_with(Instant::now(),&[1],select,&mut live);assert!(live.0.is_empty());
        let mut cut=[0;173];os(unsafe{super::super::zxs_smb_ctl_read(c.as_raw_fd(),cut.as_mut_ptr(),173)}).unwrap();
        os(unsafe{zxs_smb_active_send(c.as_raw_fd(),ch.as_ptr())}).unwrap();
        active.observe_with(Instant::now(),&[1],select,&mut live);
        assert!(active.0.is_empty());assert_eq!(live.0.len(),1);
        let s=&live.0[0];
        assert!(s.select(&src,share.share).is_err());
        assert!(s.select(&src,ShareId([0x91;16])).is_err());
        let allowed=web.set_share_mode(OperationId([0x99;16]),share.share,share.op,Mode::ReadOnly).unwrap();
        assert_eq!(s.select(&src,share.share).unwrap(),allowed);
        let off=web.set_share_mode(OperationId([0x9a;16]),share.share,allowed.op,Mode::Disabled).unwrap();
        assert!(s.select(&src,share.share).is_err());
        let again=web.set_share_mode(OperationId([0x9b;16]),share.share,off.op,Mode::ReadWrite).unwrap();
        assert_ne!(again.op,allowed.op);assert_eq!(s.select(&src,share.share).unwrap(),again);
        drop(c);live.observe(&[2],|_,_,_|panic!("closed original session must not access SQL"));
        assert!(live.0.is_empty());
    }
    #[test]
    #[ignore="source-only; original non-root synthetic SQLite pool, not a Samba logon"]
    fn only_the_original_current_smb_slot_can_receive_and_keep_its_challenge(){
        use crate::{system::reader::{InitialReader,web_pool},test_support::PrivateTemp};
        let tmp=PrivateTemp::under(&std::env::temp_dir());let (s,p)=web_pool(&tmp);
        let src=InitialReader::open(&s,p).unwrap();let mut domain=[0;24];
        domain[..12].copy_from_slice(&super::super::DOMAIN);
        for(i,v)in [1u32,2,3].into_iter().enumerate(){domain[12+4*i..16+4*i].copy_from_slice(&v.to_le_bytes());}
        let sid=super::super::expected_user(domain);
        let select=||super::super::current_slot(&src,p,[7;16],&sid);
        let mut active=Active::new();let (first,peer)=pair();
        active.admit(scope(peer),select).unwrap();
        assert_ne!(read(&first),[0;8]);assert_eq!(active.0[0].slot.original.account.0,[0x56;16]);
        let mut wrong=sid;wrong[12]^=1;let (never,peer)=pair();
        assert!(active.admit(scope(peer),||super::super::current_slot(&src,p,[7;16],&wrong)).is_err());
        assert!(unsafe{super::super::zxs_smb_ctl_live(never.as_raw_fd())}<0);
        let away=tmp.0.join("held-system-away");std::fs::rename(&s,&away).unwrap();
        active.recheck(select);assert!(active.0.is_empty());
        assert!(unsafe{super::super::zxs_smb_ctl_live(first.as_raw_fd())}<0);
        std::fs::rename(&away,&s).unwrap();let (fresh,peer)=pair();
        active.admit(scope(peer),select).unwrap();assert_ne!(read(&fresh),[0;8]);
        let different=||{let mut x=select()?;x.original.generation+=1;Ok(x)};
        active.recheck(different);assert!(active.0.is_empty()); // explicit injection for this challenge fixture; not a real reset
        assert!(unsafe{super::super::zxs_smb_ctl_live(fresh.as_raw_fd())}<0);
    }
    #[test]
    #[ignore="source-only: real non-root SQLite pool, no client password or network"]
    fn final_public_generation_is_one_shot_from_the_same_registered_attempt(){
        use crate::{system::reader::{InitialReader,web_pool},test_support::PrivateTemp};
        let tmp=PrivateTemp::under(&std::env::temp_dir());let (root,p)=web_pool(&tmp);
        let src=InitialReader::open(&root,p).unwrap();let mut domain=[0;24];
        domain[..12].copy_from_slice(&super::super::DOMAIN);
        for(i,v)in [1u32,2,3].into_iter().enumerate(){domain[12+4*i..16+4*i].copy_from_slice(&v.to_le_bytes());}
        let sid=super::super::expected_user(domain);
        let select=||super::super::current_slot(&src,p,[7;16],&sid);
        let request=|fd:&OwnedFd,ch:[u8;8]|{
            let mut frame=[0u8;24];frame[..15].copy_from_slice(b"ZXS-SAM/GNCUT-1");
            frame[16..].copy_from_slice(&ch);
            os(unsafe{super::super::zxs_smb_ctl_write(fd.as_raw_fd(),frame.as_ptr(),frame.len())}).unwrap();
        };
        let open=|a:&mut Active|{
            let(c,peer)=pair();a.admit(scope(peer),select).unwrap();let ch=read(&c);(c,ch)
        };
        let mut active=Active::new();let (one,first)=open(&mut active);let (two,second)=open(&mut active);
        request(&one,second);request(&two,second);
        active.observe(Instant::now(),&[1,1],select);
        assert_eq!(active.0.len(),1);assert!(active.0[0].cut_sent);
        assert!(unsafe{super::super::zxs_smb_ctl_live(one.as_raw_fd())}<0);
        let mut ack=[0;173];
        os(unsafe{super::super::zxs_smb_ctl_read(two.as_raw_fd(),ack.as_mut_ptr(),ack.len())}).unwrap();
        assert_eq!(&ack[..16],b"ZXS-SAM/GEN-1\0\0\0");assert_eq!(&ack[16..24],&second);
        assert_eq!(&ack[24..40],&p.0);assert_eq!(&ack[40..56],&[0x56;16]);
        assert_eq!(&ack[56..72],&[0x44;16]);assert_eq!(&ack[72..80],&1i64.to_be_bytes());
        assert_eq!(&ack[80..108],&sid);assert_eq!(&ack[109..109+usize::from(ack[108])],select().unwrap().original.login.as_slice());
        assert!(ack[109+usize::from(ack[108])..].iter().all(|&b|b==0));assert_ne!(first,second);
        request(&two,second);
        active.observe(Instant::now(),&[1],||panic!("already consumed must not read SQL"));
        assert!(active.0.is_empty());assert!(unsafe{super::super::zxs_smb_ctl_live(two.as_raw_fd())}<0);
        let(three,ch)=open(&mut active);let away=tmp.0.join("not-the-original-S");
        std::fs::rename(&root,&away).unwrap();request(&three,ch);
        active.observe(Instant::now(),&[1],select);assert!(active.0.is_empty());
        assert!(unsafe{super::super::zxs_smb_ctl_live(three.as_raw_fd())}<0);
        std::fs::rename(away,&root).unwrap();
    }
    #[test]
    fn complete_begin_with_no_received_rights_rejects_only_that_attempt(){
        let(master,server)=pair();let (old,peer)=pair();let mut active=Active::new();
        active.admit(scope(peer),synthetic).unwrap();let original=read(&old);
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
