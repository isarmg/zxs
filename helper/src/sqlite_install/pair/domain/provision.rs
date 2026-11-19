//! One fresh, fixed Samba bootstrap. No CLI and no public grant constructor:
//! neither a path, an approval string, nor classifying an empty directory
//! supplies the host authorization and authenticated capsule required below.
mod monitor;
mod return_to_host;
mod live;
use super::handoff::{HeldOrigin,OriginChannel,SyncedStore};
use crate::{fs_tools::RuntimeFile,isolated_group::Group,pinned::{Entity,Mount,Pinned},
    sqlite_install::{bad,originals::LockedSystem,request::Id}};
use rustix::{fd::{AsFd,AsRawFd,OwnedFd},fs::{FlockOperation,Mode,OFlags,RawDir,flock,openat},
    io::{FdFlags,fcntl_dupfd_cloexec,fcntl_setfd},pipe::{PipeFlags,pipe_with},
    process::{Resource,Rlimit,getrlimit,setrlimit}};
use std::{fs::File,io::{self,Seek,Write},mem::MaybeUninit,os::unix::fs::MetadataExt,
    path::Path,process::{Command,Stdio}};

const CAPSULE_USR:&str="/usr/lib/local-ops-helper/samba-4.22.11-deb13u1-v1/usr";
const LAUNCHER:&[&str]=&["usr","lib","local-ops-helper","disk-tools-v1","bin","bwrap"];
const BOOT:&[u8]=include_bytes!("../../../../resources/samba-domain/domain-bootstrap.conf");
const SCRIPT:&[u8]=include_bytes!("../../../../resources/samba-domain/fresh_guest.py");
const ENTRY:&str="import runpy; runpy.run_path('/provision.py',run_name='_local_ops_sam')['_provision_original']()";

// The sole future issuer must verify the original pool, explicit local approval,
// private immutable release closure/protected installation, exact live config
// and fixed instance origins before constructing this. No public or fallback
// constructor. Merely receiving arguments, a uid or a packaging profile is NOT
// enough. The executable actually consumed is the specific digest-verified ELF.
pub(super) struct FreshApproval<'a>{
    system:&'a LockedSystem, pool:Id,init:Id,instance_id:Id,op:Id,actor:Id,reference:Id,live_config:[u8;32],host_order:[u8;32],
    site:&'a Pinned, usr:&'a Pinned, expected_bwrap:[u8;32], uid:u32,gid:u32,
}
// Only the real monitor/reaper creates this proof for its original payload.
pub(super) struct Finished(rustix::process::Pid);
impl Finished{pub(super) fn pid(&self)->rustix::process::Pid{self.0}}
pub(super) struct Unpublished<'a>{
    approval:FreshApproval<'a>,site:Instance,original:SyncedStore,live:live::Live,
}
fn mark(fd:&File)->io::Result<(Entity,u64)>{
    Ok((Entity::of(&fd.metadata()?)?,Mount::on(fd)?.ok_or_else(||bad("missing original mount identity"))?.id))
}
fn child(parent:&File,name:&std::ffi::CStr)->io::Result<File>{
    Ok(File::from(openat(parent,name,OFlags::RDONLY|OFlags::DIRECTORY|OFlags::NONBLOCK|OFlags::CLOEXEC|OFlags::NOFOLLOW,Mode::empty())?))
}
fn named(pin:&Pinned,fd:&File)->io::Result<()>{
    // Rewalk the visible chain but always keep and operate on the original.
    let again=pin.relative_directory()?;
    if mark(&again)?!=mark(fd)?{return Err(bad("selected protected directory was replaced"));}Ok(())
}
#[derive(Clone,Copy,Eq,PartialEq)]
struct Times{links:u64,len:u64,mtime:(i64,i64),ctime:(i64,i64)}
impl Times{fn on(f:&File)->io::Result<Self>{let m=f.metadata()?;Ok(Self{links:m.nlink(),len:m.len(),
    mtime:(m.mtime(),m.mtime_nsec()),ctime:(m.ctime(),m.ctime_nsec())})}}
const FRESH_SITE:&[&[u8]]=&[b"private",b"state",b"smb.conf"];
const PRIVATE_STORE:&[&[u8]]=&[b"secrets.tdb"];
fn contents(dir:&File,expected:&[&[u8]])->io::Result<()>{
    // All lists are small fixed constants. A raw, complete inventory rejects
    // case-folded aliases and previously allocated partial or final outputs.
    if expected.len()>8 {return Err(bad("invalid dedicated directory inventory"));}
    let before=Times::on(dir)?;let other=child(dir,c".")?;
    if before.links==0||mark(&other)?!=mark(dir)?{return Err(bad("cannot attribute the original directory"));}
    let mut buf=[MaybeUninit::uninit();4096];let mut it=RawDir::new(&other,&mut buf);let mut seen=0u16;
    while let Some(row)=it.next(){let row=row?;let bytes=row.file_name().to_bytes();
        if matches!(bytes,b"."|b".."){continue;}
        let n=expected.iter().position(|&name|name==bytes).ok_or_else(||bad("unexpected entry in the selected Samba directory; not taking it over"))?;
        if seen&(1u16<<n)!=0{return Err(bad("duplicate entry in the dedicated instance inventory"));}
        seen|=1u16<<n;
    }
    if seen!=(1u16<<expected.len())-1||Times::on(&other)?!=before||Times::on(dir)?!=before{return Err(bad("incomplete or changed selected Samba directory"));}
    Ok(())
}
struct Instance{base:File,private:File,state:File}
impl Instance{
    fn lock(approval:&FreshApproval<'_>)->io::Result<Self>{
        let base=approval.site.relative_directory()?;let md=base.metadata()?;
        if md.uid()!=0||!md.is_dir()||md.mode()&0o022!=0||md.nlink()==0||mark(&base)?==mark(approval.system.source().dir())?{
            return Err(bad("dedicated instance must be separate, protected and root-owned"));}
        flock(&base,FlockOperation::NonBlockingLockExclusive)?;
        let private=child(&base,c"private")?;let state=child(&base,c"state")?;
        let s=Self{base,private,state};s.require(approval,None)?;Ok(s)
    }
    fn require(&self,a:&FreshApproval<'_>,store:Option<&HeldOrigin>)->io::Result<()>{
        a.system.require()?;named(a.site,&self.base)?;
        let m=self.base.metadata()?;if !m.is_dir()||m.uid()!=0||m.mode()&0o022!=0||m.nlink()==0{return Err(bad("unprotected installation directory"));}
        let (base,mount)=mark(&self.base)?;let system=mark(a.system.source().dir())?.0;
        let p=mark(&self.private)?.0;let s=mark(&self.state)?.0;
        if p==s||[base,system].contains(&p)||[base,system].contains(&s){return Err(bad("aliased dedicated Samba directories"));}
        for (f,n) in [(&self.private,c"private"),(&self.state,c"state")]{
            let md=f.metadata()?;
            if !md.is_dir()||md.uid()!=a.uid||md.mode()&0o7777!=0o700||md.nlink()==0||
                mark(f)?.1!=mount||mark(&child(&self.base,n)?)?!=mark(f)?{
                return Err(bad("changed or separately mounted private/state directory; not supported"));}
        }
        // Open and enumerate the actual root before any native provisioning,
        // again immediately before launch and at all subsequent cuts. Existing
        // BOUND (even if the TDB vanished), an abandoned stage, or an alias is
        // an occupied/unknown order, not a new origin.
        contents(&self.base,FRESH_SITE)?;
        contents(&self.state,&[])?;
        contents(&self.private,if store.is_some(){PRIVATE_STORE}else{&[]})?;
        if let Some(h)=store{h.recheck(&self.private)?;}
        named(a.site,&self.base)?;Ok(())
    }
}
fn cancelled()->io::Result<()>{
    if helper_linux_evidence::diagnostic_process::cancelled(){Err(io::Error::new(io::ErrorKind::Interrupted,"Samba operation interrupted; preserve original state"))}
    else{Ok(())}
}
fn limits()->io::Result<()>{
    for (r,n) in [(Resource::Cpu,20),(Resource::As,512<<20),(Resource::Fsize,16<<20),
                  (Resource::Core,0),(Resource::Nofile,128),(Resource::Nproc,64)]{
        let old=getrlimit(r);let hard=old.maximum.map_or(n,|h|h.min(n));
        let soft=old.current.map_or(hard,|s|s.min(hard));setrlimit(r,Rlimit{current:Some(soft),maximum:Some(hard)})?;
    }Ok(())
}
fn sealed(bytes:&[u8])->io::Result<File>{
    use rustix::fs::{MemfdFlags,SealFlags,fcntl_add_seals,fcntl_get_seals,memfd_create};
    let mut f=File::from(memfd_create(c"helper-sam-fixed",MemfdFlags::CLOEXEC|MemfdFlags::ALLOW_SEALING|MemfdFlags::NOEXEC_SEAL)?);
    f.write_all(bytes)?;let seals=SealFlags::WRITE|SealFlags::GROW|SealFlags::SHRINK|SealFlags::SEAL;
    fcntl_add_seals(&f,seals)?;
    if !fcntl_get_seals(&f)?.contains(seals|SealFlags::EXEC)||f.metadata()?.len()!=bytes.len() as u64{return Err(bad("incomplete fixed bootstrap artifact"));}
    f.rewind()?;Ok(f)
}
fn give(fd:&impl AsFd)->io::Result<OwnedFd>{
    let n=fcntl_dupfd_cloexec(fd,10)?;fcntl_setfd(&n,FdFlags::empty())?;Ok(n)
}
// --ro-bind-data unlinks its backing temporary: it is appropriate for the
// script but WOULD give the configuration st_nlink==0. Write the latter once
// to the private new root, then remount that root readonly before any payload.
fn fixed_args(cmd:&mut Command,uid:u32,gid:u32,ns:&[String;7]){
    cmd.args(["--unshare-user","--unshare-pid","--as-pid-1","--unshare-net","--unshare-ipc","--unshare-uts",
            "--disable-userns","--die-with-parent","--cap-drop","ALL","--clearenv","--setenv","LC_ALL","C",
            "--uid",&uid.to_string(),"--gid",&gid.to_string(),"--chdir","/",
            "--ro-bind-fd",&ns[0],"/usr","--symlink","usr/lib","/lib","--symlink","usr/lib64","/lib64",
            "--bind-fd",&ns[1],"/instance/private","--ro-bind-fd",&ns[2],"/instance/state",
            "--perms","0444","--ro-bind-data",&ns[3],"/provision.py",
            "--perms","0444","--file",&ns[4],"/instance/domain-bootstrap.conf",
            "--seccomp",&ns[5],"--json-status-fd",&ns[6],"--remount-ro","/","--",
            "/usr/bin/python3.13","-I","-S","-B","-c",ENTRY]);
}
impl<'a> FreshApproval<'a>{
    /// One-use in a dedicated just-dropped single-threaded CLI worker, NOT a web
    /// process. Host account/instance release authority is deliberately not yet issued.
    pub(super) fn provision(self)->io::Result<Unpublished<'a>>{
        use std::os::unix::fs::FileExt;
        rustix::thread::set_no_new_privs(true)?;
        crate::maintenance::diagnostic_worker(self.uid,self.gid)?;
        if self.system.source().uid()!=self.uid||self.uid==0||self.gid==0||
            [self.pool,self.init,self.instance_id,self.op,self.actor,self.reference].contains(&[0;16])||self.usr.path()!=Path::new(CAPSULE_USR){
            return Err(bad("not the selected dedicated instance operation"));}
        let instance_name=format!("/var/lib/local-ops-helper/samba-instances/{}",self.instance_id.iter().map(|n|format!("{n:02x}")).collect::<String>());
        let s=self.system.source();let real_s=Pinned::open(s.path(),crate::pinned::Kind::StorageRoot)?;
        if self.site.path()!=Path::new(&instance_name)||mark(s.dir())? != (real_s.entity(),real_s.mount()?.id)
            ||self.site.entity()==real_s.entity()||self.site.entity_is_visible_ancestor_of(&real_s)||real_s.entity_is_visible_ancestor_of(self.site)
            ||self.site.entity()==self.usr.entity()||self.site.entity_is_visible_ancestor_of(self.usr)||self.usr.entity_is_visible_ancestor_of(self.site){
            return Err(bad("dedicated instance, selected release and original SYSTEM must be independent origins"));}
        helper_linux_evidence::isolate_check_descriptors()?;
        crate::isolated_group::prepare()?;limits()?;
        let mut bwrap=RuntimeFile::open(LAUNCHER,true,32<<20)?;
        let usr=self.usr.relative_directory()?;let m=usr.metadata()?;
        if m.uid()!=0||m.mode()&0o022!=0||m.nlink()==0||!m.is_dir()||bwrap.digest()?!=self.expected_bwrap{
            return Err(bad("not the approved dedicated installation release"));}
        let mut elf=[0;20];bwrap.file.read_exact_at(&mut elf,0)?;
        if &elf[..7]!=b"\x7fELF\x02\x01\x01"||elf[18..20]!=[62,0]{return Err(bad("not the selected x86-64 launcher"));}
        // Before exposing a writable empty private directory to Samba, query only the
        // explicitly selected original S under its EX; no SQL connection survives
        // the native process. A pool ID supplied by the caller alone is not enough.
        let site=Instance::lock(&self)?;
        let live=live::Live::select(&site.base,&self.live_config)?;
        let check_order=||->io::Result<()>{
            let dig=return_to_host::order([&self.pool,&self.init,&self.instance_id,&self.op,&self.actor,&self.reference],
                self.uid,self.gid,s.path(),&self.live_config,[mark(s.dir())?,mark(&site.base)?,mark(&site.private)?,mark(&site.state)?,live.origin()])?;
            if dig!=self.host_order{return Err(bad("foreground host approval referred to different original objects; not provisioning"));}
            Ok(())
        };
        check_order()?;
        // Only now open the SQL file: a wrong selected inode is not even read.
        super::super::observed::original_pool(&self.pool,&self.init,self.system)?;
        let (origin,tx)=OriginChannel::open()?;let (rstatus,wstatus)=pipe_with(PipeFlags::CLOEXEC)?;
        let (stdin,fin)=pipe_with(PipeFlags::CLOEXEC)?;drop(fin);
        let (script,boot,bpf)=(sealed(SCRIPT)?,sealed(BOOT)?,sealed(&helper_linux_evidence::diagnostic_process::diagnostic_group_filter())?);
        let fds=[give(&usr)?,give(&site.private)?,give(&site.state)?,give(&script)?,give(&boot)?,give(&bpf)?,give(&wstatus)?];
        let ns=fds.each_ref().map(|f|f.as_raw_fd().to_string());
        let mut cmd=Command::new("/no-path-execution-for-helper-sam");
        cmd.env_clear().current_dir("/").stdin(Stdio::from(File::from(stdin))).stdout(Stdio::from(tx)).stderr(Stdio::piped());
        fixed_args(&mut cmd,self.uid,self.gid,&ns);
        named(self.usr,&usr)?;bwrap.recheck()?;site.require(&self,None)?;live.require(&site.base)?;check_order()?;cancelled()?;
        helper_linux_evidence::diagnostic_process::prepare_diagnostic_exec(&mut cmd,&bwrap.file)?;
        let group=Group::new(cmd.spawn()?);
        drop(cmd);drop(fds);drop(wstatus);
        let finished=monitor::reap(group,File::from(rstatus))?;
        cancelled()?;named(self.usr,&usr)?;bwrap.recheck()?;
        let h=origin.receive(&site.private,self.uid,self.gid,finished.pid())?;
        site.require(&self,Some(&h))?;live.require(&site.base)?;
        let h=h.sync_finished(&site.private,&finished)?;
        site.require(&self,Some(h.require(&site.private)?))?;live.require(&site.base)?;cancelled()?;
        Ok(Unpublished{approval:self,site,original:h,live})
    }
}

impl Unpublished<'_>{
    /// Nonsecret candidate for the host parent, not a persistent publication.
    /// This value cannot be reached from a user-supplied SID or rebooted TDB.
    pub(super) fn original_record(&self)->io::Result<[u8;128]>{
        let a=&self.approval;let origin=self.original.require(&self.site.private)?;self.site.require(a,Some(origin))?;self.live.require(&self.site.base)?;
        let bytes=origin.domain.encode_binding(&a.pool,&a.instance_id,&a.op,&a.live_config)?;
        self.live.require(&self.site.base)?;self.site.require(a,Some(self.original.require(&self.site.private)?))?;cancelled()?;Ok(bytes)
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use rustix::fs::{SealFlags,fcntl_get_seals};
    use std::{io::Read,os::unix::ffi::OsStrExt};
    #[test]
    fn fixed_guest_and_configuration_are_sealed_non_executable_and_rewound(){
        for bytes in [BOOT,SCRIPT]{
            let mut f=sealed(bytes).unwrap();
            assert!(fcntl_get_seals(&f).unwrap().contains(SealFlags::EXEC|SealFlags::WRITE|SealFlags::GROW|SealFlags::SHRINK|SealFlags::SEAL));
            let mut observed=Vec::new();f.read_to_end(&mut observed).unwrap();assert_eq!(observed,bytes);
            assert!(f.write_all(b"x").is_err());
        }
    }
    #[test]
    fn config_is_a_linked_mode_0444_file_on_the_new_root_made_readonly_before_exec(){
        let ids=[10,11,12,13,14,15,16].map(|n|n.to_string());
        let mut c=Command::new("/unused");fixed_args(&mut c,1001,1002,&ids);
        let a:Vec<&str>=c.get_args().map(|s|s.to_str().unwrap()).collect();
        let pos=|q:&[&str]|a.windows(q.len()).position(|v|v==q).unwrap();
        let write=pos(&["--perms","0444","--file","14","/instance/domain-bootstrap.conf"]);
        assert!(write<pos(&["--remount-ro","/"]));
        assert!(pos(&["--remount-ro","/"])<pos(&["--","/usr/bin/python3.13"]));
        assert!(pos(&["--perms","0444","--ro-bind-data","13","/provision.py"])<write);
        assert!(pos(&["--bind-fd","11","/instance/private"])<write);
        assert!(pos(&["--ro-bind-fd","12","/instance/state"])<write);
        assert!(a.contains(&"--as-pid-1")&&!a.contains(&"--new-session")&&a.len()<=62);
        assert_eq!(a.iter().filter(|&&x|x=="--bind-fd").count(),1);
    }
    #[test]
    fn actual_private_directory_accepts_only_the_one_exact_raw_name(){
        let d=tempfile::tempdir().unwrap();let f=File::open(d.path()).unwrap();
        contents(&f,&[]).unwrap();assert!(contents(&f,PRIVATE_STORE).is_err());
        let alt=d.path().join(std::ffi::OsStr::from_bytes(b"secrets.tdb\xff"));File::create_new(&alt).unwrap();
        assert!(contents(&f,&[]).is_err());assert!(contents(&f,PRIVATE_STORE).is_err());
        std::fs::rename(&alt,d.path().join("secrets.tdb")).unwrap();contents(&f,PRIVATE_STORE).unwrap();
        File::create_new(&alt).unwrap();assert!(contents(&f,PRIVATE_STORE).is_err());
    }
    #[test]
    fn existing_bound_or_unfinished_staging_refuses_a_fresh_selection_even_with_empty_private(){
        let d=tempfile::tempdir().unwrap();let root=File::open(d.path()).unwrap();
        for n in ["private","state"]{std::fs::create_dir(d.path().join(n)).unwrap();}
        File::create_new(d.path().join("smb.conf")).unwrap(); contents(&root,FRESH_SITE).unwrap();
        for name in ["domain.bound","DOMAIN.BOUND",".domain.bound.new","other"]{
            let p=d.path().join(name);std::fs::write(&p,b"preserve me").unwrap();
            assert!(contents(&root,FRESH_SITE).is_err());assert_eq!(std::fs::read(&p).unwrap(),b"preserve me");
            assert!(!d.path().join("private/secrets.tdb").exists());std::fs::remove_file(&p).unwrap();
        }
        contents(&root,FRESH_SITE).unwrap();
    }
}
