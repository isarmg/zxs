//! A second, private, one-use socketpair. Only this operation's lowered
//! worker sends the public result and transfers its *existing* two EX open
//! descriptions. No pathname, writable output FD, TDB contents or root
//! directory authority is granted to the worker. Not an online smbd channel.
mod issue;
mod worker;
mod installed;
mod layout;
pub(in crate::sqlite_install::pair) use layout::create_layout;
pub(in crate::sqlite_install::pair) use installed::{InstalledSetup,SelectedSetup};
mod start;
pub(crate) use worker::exec_child;
use super::{FRESH_SITE,PRIVATE_STORE,Unpublished,child,contents,live,mark};
use super::super::{LocalDomain,handoff::{OriginChannel,SyncedStore}};
use crate::{pinned::{Entity,Kind,Pinned},sqlite_install::{approval,bad,request::Id}};
use rustix::{fd::AsFd,fs::{FlockOperation,Mode,OFlags,fcntl_getfl,flock,openat},io::Errno,
    net::{AddressFamily,SendAncillaryBuffer,SendAncillaryMessage,SendFlags,SocketType,UCred,sendmsg,sockopt},
    process::{Gid,Pid,Uid,WaitId,WaitIdOptions,getppid,waitid}};
use std::{ffi::CString,fs::File,io::{self,IoSlice,Read},mem::MaybeUninit,os::{fd::AsRawFd,unix::fs::MetadataExt},path::Path,process::Child};

const RESULT:usize=192; // 128 B BOUND + confirmed host scope (includes original INIT) + sync comparison
const PREFIX:[&std::ffi::CStr;4]=[c"var",c"lib",c"local-ops-helper",c"samba-instances"];
// A nonsecret local comparison, not a signature. Parent and worker both use
// their independently selected, simultaneously live original objects; do not
// persist dev/mount/inode as reboot-stable IDs. Only the parent's explicit
// terminal confirmation authorizes this single future installation.
pub(super) fn order(ids:[&Id;6],uid:u32,gid:u32,path:&Path,config:&[u8;32],release:&[u8;32],objs:[(Entity,u64);6])->io::Result<[u8;32]>{
    use sha2::{Digest,Sha256};use std::os::unix::ffi::OsStrExt;
    let mut h=Sha256::new();h.update(b"helper: one original host SAM installation v2\0");
    for id in ids{if *id==[0;16]{return Err(bad("missing host installation identity"));}h.update(id);}
    h.update(uid.to_be_bytes());h.update(gid.to_be_bytes());let p=path.as_os_str().as_bytes();
    h.update(u16::try_from(p.len()).map_err(|_|bad("out-of-range installation root"))?.to_be_bytes());h.update(p);
    h.update(config);h.update(release);for (e,m) in objs{h.update(e.dev.to_be_bytes());h.update(e.ino.to_be_bytes());h.update(m.to_be_bytes());}
    Ok(h.finalize().into())
}
fn safe_dir(f:&File)->io::Result<()>{
    let m=f.metadata()?;
    if !m.is_dir()||m.uid()!=0||m.nlink()==0||m.mode()&0o022!=0{return Err(bad("unprotected host instance ancestor"));}Ok(())
}
fn pin_dir(parent:&File,n:&std::ffi::CStr)->io::Result<File>{
    Ok(File::from(openat(parent,n,OFlags::PATH|OFlags::DIRECTORY|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?))
}
struct Site{host:File,chain:Vec<File>,leaf:CString}
impl Site{
    fn select(host:File,instance:&Id)->io::Result<Self>{
        Self::walk(host,instance,false)
    }
    // Only layout preparation may stop at the first absent component below
    // an already verified /var/lib. All existing operational users require
    // the complete protected chain, including the instance itself.
    fn walk(host:File,instance:&Id,missing_suffix:bool)->io::Result<Self>{
        let leaf=CString::new(instance.iter().map(|b|format!("{b:02x}")).collect::<String>()).expect("lower hex");
        let mut chain=Vec::with_capacity(PREFIX.len()+1);safe_dir(&host)?;
        for name in PREFIX.iter().copied().chain(std::iter::once(leaf.as_c_str())){
            let original=match pin_dir(chain.last().unwrap_or(&host),name){
                Err(e) if missing_suffix && chain.len()>=2 && e.kind()==io::ErrorKind::NotFound=>break,
                r=>r?,
            };
            safe_dir(&original)?;chain.push(original);
        }
        let this=Self{host,chain,leaf};this.check()?;Ok(this)
    }
    fn base(&self)->&File{self.chain.last().expect("base retained")}
    fn check(&self)->io::Result<()>{
        self.require_from(approval::host_admin()?)
    }
    // Keep the original root selected before permanently lowering identity.
    // Rewalk from the actual current "/" without administrative privileges.
    fn require_from(&self,host:File)->io::Result<()>{
        safe_dir(&self.host)?;safe_dir(&host)?;
        if mark(&self.host)?!=mark(&host)?{return Err(bad("different current host root"));}
        let mut p=host;
        for (original,n) in self.chain.iter().zip(PREFIX.iter().copied().chain(std::iter::once(self.leaf.as_c_str()))){
            let next=pin_dir(&p,n)?;safe_dir(original)?;safe_dir(&next)?;
            if mark(original)?!=mark(&next)?{return Err(bad("protected installation path changed"));}
            p=next;
        }Ok(())
    }
}
// Object selection is not the operator's authority. A later, dedicated
// foreground issuer must separately confirm the action and identified pool.
pub(super) struct HostOrigins{
    system:Pinned,site:Site,private:File,state:File,live:live::Live,release:super::capsule::Originals,
    uid:u32,gid:u32,pool:Id,init:Id,instance:Id,op:Id,actor:Id,config:[u8;32],
}
impl HostOrigins{
    pub(super) fn before_worker(path:&Path,pool:Id,init:Id,instance:Id,op:Id,config:[u8;32])->io::Result<Self>{
        approval::no_inherited_descriptors()?;
        if [pool,init,instance,op].contains(&[0;16]){return Err(bad("missing original installation identities"));}
        let host=approval::host_admin()?;let (actor,uid,gid)=approval::profile()?;
        let system=Pinned::open(path,Kind::StorageRoot)?;let site=Site::select(host,&instance)?;
        let selected=|n:&std::ffi::CStr|->io::Result<File>{
            let p=pin_dir(site.base(),n)?;let m=p.metadata()?;
            if !m.is_dir()||m.uid()!=uid||m.mode()&0o7777!=0o700||m.nlink()==0||mark(&p)?.1!=mark(site.base())?.1{
                return Err(bad("not an original storage-owned installation directory"));}
            let f=child(&p,c".")?;
            if mark(&f)?!=mark(&p)?||mark(&pin_dir(site.base(),n)?)?!=mark(&p)?{return Err(bad("private/state changed during admission"));}
            Ok(f)
        };
        let private=selected(c"private")?;let state=selected(c"state")?;
        let live=live::Live::select(site.base(),&config)?;
        let release=super::capsule::Originals::open()?;
        let this=Self{system,site,private,state,live,release,uid,gid,pool,init,instance,op,actor,config};
        this.fresh()?;Ok(this)
    }
    fn common(&self)->io::Result<()>{
        self.site.check()?;self.system.require_same_path()?;self.release.slots()?;
        let (a,u,g)=approval::profile()?;
        let s=self.system.metadata()?;let root=self.site.base();
        if (a,u,g)!=(self.actor,self.uid,self.gid)||!s.is_dir()||s.uid()!=u||s.mode()&0o7777!=0o700||s.nlink()==0{
            return Err(bad("inconsistent storage subject, SYSTEM or protected instance"));}
        for f in &self.site.chain {
            if mark(f)?.0==self.system.entity(){return Err(bad("SYSTEM is an ancestor of the dedicated installation"));}
        }
        if self.system.visible_chain().contains(&mark(root)?.0){return Err(bad("dedicated installation contains SYSTEM"));}
        let ids=[self.system.entity(),mark(root)?.0,mark(&self.private)?.0,mark(&self.state)?.0];
        if ids.iter().enumerate().any(|(i,v)|ids[..i].contains(v)){return Err(bad("overlapping instance roles"));}
        let mount=mark(root)?.1;
        for (dir,n) in [(&self.private,c"private"),(&self.state,c"state")]{
            let m=dir.metadata()?;
            if !m.is_dir()||m.uid()!=u||m.mode()&0o7777!=0o700||m.nlink()==0
                ||mark(dir)?.1!=mount||mark(&pin_dir(root,n)?)?!=mark(dir)?{return Err(bad("changed private/state original"));}
        }
        self.live.require(root)?;contents(&self.state,&[])?;Ok(())
    }
    fn marks(&self)->io::Result<[(Entity,u64);6]>{
        Ok([(self.system.entity(),self.system.mount()?.id),mark(self.site.base())?,mark(&self.private)?,mark(&self.state)?,self.live.origin(),self.release.original()?])
    }
    fn scope(&self,reference:&Id)->io::Result<[u8;32]>{
        order([&self.pool,&self.init,&self.instance,&self.op,&self.actor,reference],self.uid,self.gid,
              self.system.path(),&self.config,&self.release.key(),self.marks()?)
    }
    pub(super) fn issue(&self,reference:Id)->io::Result<[u8;128]>{issue::foreground(self,reference)}
    pub(super) fn fresh(&self)->io::Result<()>{
        self.common()?;contents(self.site.base(),FRESH_SITE)?;contents(&self.private,&[])?;self.site.check()
    }
}
impl Unpublished<'_>{
    pub(super) fn send_to_host(self,tx:&File)->io::Result<()>{
        let who=sockopt::socket_peercred(tx)?;
        if sockopt::socket_domain(tx)?!=AddressFamily::UNIX||sockopt::socket_type(tx)?!=SocketType::SEQPACKET
            ||Some(who.pid)!=getppid()||!who.uid.is_root()||!who.gid.is_root(){
            return Err(bad("the return endpoint does not belong to the original host parent"));}
        let mut bytes=[0;RESULT];bytes[..128].copy_from_slice(&self.original_record()?);
        let (store,witness)=self.original.feedback(&self.site.private)?;bytes[128..160].copy_from_slice(&self.approval.host_order);bytes[160..].copy_from_slice(&witness);
        let fds=[self.approval.system.source().dir().as_fd(),self.site.base.as_fd(),store.as_fd()];
        let mut buf=[MaybeUninit::uninit();rustix::cmsg_space!(ScmRights(3))];
        let mut control=SendAncillaryBuffer::new(&mut buf);
        if !control.push(SendAncillaryMessage::ScmRights(&fds)){return Err(bad("no room for original handoff"));}
        if sendmsg(tx,&[IoSlice::new(&bytes)],&mut control,SendFlags::DONTWAIT|SendFlags::NOSIGNAL)?!=RESULT{
            return Err(bad("incomplete original worker handoff"));}
        // No explicit unlock. SCM_RIGHTS holds the open descriptions even
        // if the worker exits before its parent reads the bounded packet.
        Ok(())
    }
}
// Linux show_fd_locks emits an entry only when the kernel file pointer is
// exactly this open description (not merely the same inode). A lock owner
// can have exited; the PID field and superblock device are not durable IDs.
fn owns_flock(data:&[u8],inode:u64,mount:u64)->bool{
    let Ok(text)=std::str::from_utf8(data)else{return false;};
    if text.len()>4096||!text.ends_with('\n'){return false;}
    let mut got=[false;3];
    for row in text.lines(){
        let Some((key,value))=row.split_once(':')else{return false;};
        let value=value.trim_ascii();let index=match key{
            "mnt_id" if value.parse::<u64>()==Ok(mount)=>0,
            "ino" if value.parse::<u64>()==Ok(inode)=>1,
            "lock"=>{
                let fields:Vec<_>=value.split_ascii_whitespace().collect();
                let [ordinal,"FLOCK","ADVISORY","WRITE",pid,device,"0","EOF"]=fields.as_slice() else{return false;};
                if *ordinal!="1:"||pid.parse::<i32>().is_err(){return false;}
                let mut dev=device.split(':');
                let hex=|s:Option<&str>|s.is_some_and(|v|!v.is_empty()&&v.len()<=8&&v.bytes().all(|b|b.is_ascii_hexdigit()));
                if !hex(dev.next())||!hex(dev.next())||dev.next().and_then(|n|n.parse::<u64>().ok())!=Some(inode)||dev.next().is_some(){return false;}2
            }
            "mnt_id"|"ino"=>return false,_=>continue,
        };
        if std::mem::replace(&mut got[index],true){return false;}
    }
    got==[true;3]
}
fn require_owned(fd:&File,id:(Entity,u64))->io::Result<()>{
    let path=CString::new(format!("thread-self/fdinfo/{}",fd.as_raw_fd())).expect("decimal descriptor");
    let mut bytes=Vec::new();crate::maintenance::thread_file(&path)?.take(4097).read_to_end(&mut bytes)?;
    if !owns_flock(&bytes,id.0.ino,id.1){return Err(bad("no exact whole-file exclusive flock on the received open description"));}
    Ok(())
}
fn lease(fd:&File,id:(Entity,u64))->io::Result<()>{
    let flags=fcntl_getfl(fd)?;
    if flags.contains(OFlags::PATH)||!flags.contains(OFlags::DIRECTORY)||flags&OFlags::ACCMODE!=OFlags::RDONLY
        ||mark(fd)?!=id||!fd.metadata()?.is_dir(){return Err(bad("not the expected original worker lease"));}
    require_owned(fd,id)?;
    // Separately this independent OFD must lose even a SH request; it must never
    // reacquire or convert the alleged passed EX to manufacture continuity.
    let other=child(fd,c".")?;
    if mark(&other)?!=id{return Err(bad("lease original changed"));}
    match flock(&other,FlockOperation::NonBlockingLockShared){
        Err(Errno::WOULDBLOCK)=>require_owned(fd,id),Err(e)=>Err(e.into()),Ok(())=>Err(bad("original lease is no longer exclusive")),
    }
}
pub(super) struct Returned<'a>{
    originals:&'a HostOrigins,system:File,base:File,store:File,record:[u8;128],scope:[u8;32],witness:[u8;32],
}
// WNOWAIT anchors this child through the kernel-credential check. No group or
// unrelated descriptor can supply the actual wait result.
fn waitable(child:&Child)->io::Result<Option<(Pid,bool)>>{
    let pid=Pid::from_raw(i32::try_from(child.id()).map_err(|_|bad("invalid original child identity"))?)
        .ok_or_else(||bad("invalid original child PID"))?;
    loop{match waitid(WaitId::Pid(pid),WaitIdOptions::EXITED|WaitIdOptions::NOHANG|WaitIdOptions::NOWAIT){
        Err(Errno::INTR)=>continue,r=>return Ok(r?.map(|s|(pid,s.exit_status()==Some(0)))),
    }}
}
impl<'a> Returned<'a>{
    // The caller is the private one-shot supervisor, which must have spawned
    // the fixed protected executable itself. Child PID is observed while the
    // real child is still waitable, not supplied by argv, a parsed result or
    // a later lookup. This method does not reap or kill that child.
    fn from_waitable(rx:OriginChannel,pid:Pid,originals:&'a HostOrigins)->io::Result<Self>{
        let (data,[system,base,store])=rx.exact::<RESULT,3>(UCred{pid,uid:Uid::from_raw(originals.uid),gid:Gid::from_raw(originals.gid)})?;
        let this=Self{originals,system,base,store,record:data[..128].try_into().expect("fixed BOUND"),scope:data[128..160].try_into().expect("fixed approved host scope"),witness:data[160..].try_into().expect("fixed witness")};
        this.require()?;Ok(this)
    }
    fn require(&self)->io::Result<()>{self.require_names(FRESH_SITE)}
    fn require_names(&self,names:&[&[u8]])->io::Result<()>{
        let s=self.originals;s.common()?;contents(s.site.base(),names)?;contents(&s.private,PRIVATE_STORE)?;
        let selected=s.system.relative_directory()?;
        lease(&self.system,mark(&selected)?)?;lease(&self.base,mark(s.site.base())?)?;
        LocalDomain::expected_binding(&self.record,&s.pool,&s.instance,&s.op,&s.config)?;
        SyncedStore::verify_feedback(&self.store,&s.private,s.uid,&self.witness)?;
        s.site.check()
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use rustix::net::SendAncillaryBuffer;
    fn send(tx:&File,data:&[u8],fs:&[&File]){
        let refs=fs.iter().map(|f|f.as_fd()).collect::<Vec<_>>();
        let mut buf=[MaybeUninit::uninit();rustix::cmsg_space!(ScmRights(4))];
        let mut c=SendAncillaryBuffer::new(&mut buf);
        assert!(c.push(SendAncillaryMessage::ScmRights(&refs)));
        assert_eq!(sendmsg(tx,&[IoSlice::new(data)],&mut c,SendFlags::NOSIGNAL).unwrap(),data.len());
    }
    fn local()->UCred{UCred{pid:rustix::process::getpid(),uid:rustix::process::getuid(),gid:rustix::process::getgid()}}
    #[test]
    fn two_original_locks_stay_held_in_flight_and_after_receipt_until_the_receiver_releases_each(){
        let d=tempfile::tempdir().unwrap();let original=File::open(d.path()).unwrap();
        for n in ["system","instance"]{std::fs::create_dir(d.path().join(n)).unwrap();}
        std::fs::write(d.path().join("public-test-origin"),b"fixture only").unwrap();
        let sys=child(&original,c"system").unwrap();let site=child(&original,c"instance").unwrap();
        let probe=child(&original,c"system").unwrap();let independent=child(&original,c"instance").unwrap();
        flock(&sys,FlockOperation::NonBlockingLockExclusive).unwrap();flock(&site,FlockOperation::NonBlockingLockExclusive).unwrap();
        let tdb=File::from(openat(&original,c"public-test-origin",OFlags::PATH|OFlags::CLOEXEC|OFlags::NOFOLLOW,Mode::empty()).unwrap());
        let (rx,tx)=OriginChannel::open().unwrap();send(&tx,&[5;RESULT],&[&sys,&site,&tdb]);
        drop(sys);drop(site);drop(tdb);drop(tx); // The socket is the only owner of the two exclusive descriptions.
        for p in [&probe,&independent]{assert_eq!(flock(p,FlockOperation::NonBlockingLockShared),Err(Errno::WOULDBLOCK));}
        let (b,[ret_sys,ret_site,store])=rx.exact::<RESULT,3>(local()).unwrap();
        assert_eq!(b,[5;RESULT]);assert!(fcntl_getfl(&store).unwrap().contains(OFlags::PATH));
        lease(&ret_sys,mark(&probe).unwrap()).unwrap();lease(&ret_site,mark(&independent).unwrap()).unwrap();
        assert!(lease(&ret_sys,mark(&independent).unwrap()).is_err());drop(ret_sys);
        flock(&probe,FlockOperation::NonBlockingLockShared).unwrap();
        assert_eq!(flock(&independent,FlockOperation::NonBlockingLockShared),Err(Errno::WOULDBLOCK));
        drop(ret_site);flock(&independent,FlockOperation::NonBlockingLockShared).unwrap();
    }
    #[test]
    fn simply_passing_the_right_directory_is_not_an_exclusive_lease(){
        let d=tempfile::tempdir().unwrap();let original=File::open(d.path()).unwrap();
        let own=child(&original,c".").unwrap();let id=mark(&own).unwrap();
        assert!(lease(&own,id).is_err());flock(&own,FlockOperation::NonBlockingLockShared).unwrap();assert!(lease(&own,id).is_err());
        flock(&own,FlockOperation::NonBlockingLockExclusive).unwrap();lease(&own,id).unwrap();
        let wrong=child(&own,c".").unwrap();assert_eq!(mark(&wrong).unwrap(),id);
        // The original separately held EX really does block a new SH on the
        // other open description; this earlier predicate ALONE is insufficient.
        assert_eq!(flock(&wrong,FlockOperation::NonBlockingLockShared),Err(Errno::WOULDBLOCK));
        assert!(lease(&wrong,id).is_err());lease(&own,id).unwrap();
    }
    #[test]
    fn only_the_original_fdinfo_flock_write_and_whole_range_count(){
        let s="pos:\t0\nflags:\t0300000\nmnt_id:\t99\nino:\t101\nlock:\t1: FLOCK  ADVISORY  WRITE 0 00:6a:101 0 EOF\n";
        assert!(owns_flock(s.as_bytes(),101,99));
        for bad in [s.replace("FLOCK","OFDLCK"),s.replace("WRITE","READ"),s.replace("0 EOF","1 EOF"),
            s.replace("6a:101","6a:102"),s.replace("99","97"),s.replace("1: FLOCK","1: -> FLOCK"),s.replace(" 0 00:"," _ 00:"),
            s.to_owned()+"ino: 101\n",s.to_owned()+"lock: 2: FLOCK ADVISORY WRITE 61 00:6a:101 0 EOF\n"]{
            assert!(!owns_flock(bad.as_bytes(),101,99));}
    }
    #[test]
    fn the_foreground_scope_cannot_move_between_actors_roots_or_operations(){
        let a=[[1;16],[2;16],[3;16],[4;16],[5;16],[6;16]];let roots=std::array::from_fn(|i|(Entity{dev:10,ino:20+i as u64},30));
        let take=|a:&[Id;6],r:[(Entity,u64);6],p:&Path,g|order(a.each_ref(),1001,g,p,&[42;32],&[21;32],r).unwrap();
        let p=Path::new("/selected");let same=take(&a,roots,p,99);
        assert_eq!(same,take(&a,roots,p,99));
        assert_ne!(same,order(a.each_ref(),1001,99,p,&[42;32],&[22;32],roots).unwrap());
        for i in 0..a.len(){let mut other=a;other[i][0]^=1;assert_ne!(same,take(&other,roots,p,99));}
        for i in 0..roots.len(){let mut other=roots;other[i].0.ino+=1;assert_ne!(same,take(&a,other,p,99));
            let mut other=roots;other[i].1+=1;assert_ne!(same,take(&a,other,p,99));}
        assert_ne!(same,take(&a,roots,Path::new("/other"),99));assert_ne!(same,take(&a,roots,p,98));
    }

}
