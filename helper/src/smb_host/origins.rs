//! Live, independently selected installation objects for one approved start.
//! This is not an authority issuer. No "trust these hashes" CLI or callback.
use super::{Image,no,process,release::Release};
use crate::{sqlite_install::Online,pinned::{Entity,Mount,Pinned},smb_profile::{self,first::Plan}};
use helper_linux_evidence::smb_spawn::{Born,StorageConfig};
use rustix::{fs::{Mode,OFlags,PROC_SUPER_MAGIC,ResolveFlags,StatVfsMountFlags,fcntl_getfl,fstatfs,fstatvfs,open,openat2},io::Errno};
#[cfg(test)]
use rustix::fs::{FlockOperation,flock};
use sha2::{Digest,Sha256};
use std::{fs::File,io,os::unix::fs::{FileExt,MetadataExt}};

const SAFE:ResolveFlags=ResolveFlags::BENEATH.union(ResolveFlags::NO_SYMLINKS).union(ResolveFlags::NO_MAGICLINKS);
const DIR:OFlags=OFlags::PATH.union(OFlags::DIRECTORY).union(OFlags::CLOEXEC).union(OFlags::NOFOLLOW);

/// Held by the foreground issuer after its real native SQL observation
/// and the operator's original local grant. Never from a preflight or web request.
pub(super) struct Inputs<'a>{
    pub(super) approved:&'a Online,pub(super) release:&'a Release,
    pub(super) daemon_root:&'a File,pub(super) storage_root:&'a File,
    pub(super) site:&'a File,pub(super) system:&'a File,pub(super) system_pin:&'a Pinned,
    pub(super) bound_readonly:&'a File,pub(super) system_path:&'a str,
    pub(super) uid:u32,pub(super) gid:u32,pub(super) pool:[u8;16],pub(super) instance:[u8;16],
    pub(super) operation:[u8;16],pub(super) bound_sha:[u8;32],pub(super) live_sha:[u8;32],
    pub(super) plan:&'a Plan,pub(super) null:&'a File,
    pub(super) approved_nss_sha:[u8;32],
}
pub(super) fn file_at(f:&File,path:&str,flags:OFlags)->io::Result<File>{Ok(File::from(openat2(f,path,flags,Mode::empty(),SAFE)?))}
fn dir(f:&File,p:&str)->io::Result<File>{file_at(f,p,DIR)}
pub(super) fn ident(f:&File)->io::Result<Entity>{Entity::of(&f.metadata()?)}
pub(super) fn mid(f:&File)->io::Result<u64>{Ok(Mount::on(f)?.ok_or_else(no)?.id)}
pub(super) fn ro(f:&File)->io::Result<bool>{Ok(fstatvfs(f)?.f_flag.contains(StatVfsMountFlags::RDONLY))}
fn directory(f:&File,unowned:bool)->io::Result<()>{
    let m=f.metadata()?;if !m.is_dir() ||unowned&&(m.uid()!=0||m.mode()&0o022!=0){return Err(no());}Ok(())
}
fn base_view(f:&File,host:&File)->io::Result<()>{
    directory(f,true)?;
    if !ro(f)?||ident(f)?==ident(host)?||Mount::on(f)?.is_none_or(|m|m.at_root!=Some(true)){return Err(no());}
    Ok(())
}
fn prepared_view(f:&File)->io::Result<()>{
    let leaf=dir(f,"proc")?;directory(&leaf,true)?;if mid(&leaf)?!=mid(f)?{return Err(no());}Ok(())
}

// A view FD is not a retained snapshot of its unmounted leaves. Linux has
// moved the same vfsmount to this child's private namespace, so subsequent
// path lookups through it see the child's fixed proc/scratch overmounts.
#[derive(PartialEq,Eq)]
pub(super) struct OriginalMounts{root:(Entity,u64),proc:(Entity,u64),scratch:Option<[(Entity,u64);3]>}
const SCRATCH:[&str;3]=["run","cache","lock"];
const ISOLATED:StatVfsMountFlags=StatVfsMountFlags::NODEV.union(StatVfsMountFlags::NOSUID).union(StatVfsMountFlags::NOEXEC);
#[derive(Clone,Copy)]
enum Leaf{Proc,Scratch}
fn leaf_policy(kind:Leaf,magic:u32,mode:u32,uid:u32,gid:u32,ino:u64,flags:StatVfsMountFlags)->bool{
    if uid!=0||gid!=0||!flags.contains(ISOLATED){return false;}
    match kind{
        Leaf::Proc=>magic==PROC_SUPER_MAGIC as u32&&ino==1&&flags.contains(StatVfsMountFlags::RDONLY),
        Leaf::Scratch=>magic==0x0102_1994&&mode==0o040700&&!flags.contains(StatVfsMountFlags::RDONLY),
    }
}
fn attached(f:&File,kind:Leaf)->io::Result<(Entity,u64)>{
    let m=f.metadata()?;let e=Entity::of(&m)?;
    let mount=Mount::on(f)?.filter(|m|m.at_root==Some(true)).ok_or_else(no)?;
    if !m.is_dir()||m.nlink()==0||!leaf_policy(kind,fstatfs(f)?.f_type as u32,m.mode(),m.uid(),m.gid(),e.ino,
        fstatvfs(f)?.f_flag){return Err(no());}
    Ok((e,mount.id))
}
impl OriginalMounts{
    pub(super) fn capture(original:&Born,view:&File,daemon:bool)->io::Result<Self>{
        let root=process::role_root(original.original(),view)?;
        let base=(ident(&root)?,mid(&root)?);
        let p=attached(&dir(&root,"proc")?,Leaf::Proc)?;
        let scratch=if daemon{
            let i=dir(&root,"instance")?;
            let observe=|name|->io::Result<(Entity,u64)>{attached(&dir(&i,name)?,Leaf::Scratch)};
            Some([observe(SCRATCH[0])?,observe(SCRATCH[1])?,observe(SCRATCH[2])?])
        }else{None};
        let mut unique=vec![base,p];
        if let Some(extra)=scratch{unique.extend(extra);}
        if unique.iter().enumerate().any(|(i,(o,m))|unique[..i].iter().any(|(before,n)|before==o||n==m)){
            return Err(no());
        }
        process::role_root(original.original(),view)?;
        Ok(Self{root:base,proc:p,scratch})
    }
}
#[derive(Clone,Copy)]
pub(super) enum View<'a>{Prepared,Running(&'a Born,&'a OriginalMounts)}
impl View<'_>{
    fn require(self,view:&File,host:&File,daemon:bool)->io::Result<()>{
        base_view(view,host)?;
        match self{
            Self::Prepared=>prepared_view(view),
            Self::Running(id,expected) if OriginalMounts::capture(id,view,daemon)?==*expected=>Ok(()),
            _=>Err(no()),
        }
    }
}
fn exact_public(f:&File,len:u64)->io::Result<()>{
    let m=f.metadata()?;
    if !m.is_file()||m.uid()!=0||m.gid()!=0||m.nlink()!=1||m.len()!=len||m.mode()&0o7777!=0o444{return Err(no());}Ok(())
}
pub(super) fn projected(site:&File,role:&File,name:&str,len:u64)->io::Result<File>{
    let pin=OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC;
    let raw=file_at(site,name,pin)?;let show=file_at(role,name,pin)?;
    exact_public(&raw,len)?;exact_public(&show,len)?;
    if mid(&raw)?!=mid(site)?||ident(&raw)?!=ident(&show)?||mid(&raw)?==mid(&show)?||!ro(&show)?{return Err(no());}
    // Only after pinning and validating both: read from this immutable role
    // projection, never from a client path or the original secret SAM database.
    let f=crate::pinned::sam_public_reader(&show,mid(&show)?,len)?;
    exact_public(&f,len)?;
    if !super::same(&f,&show)?||!ro(&f)?||!super::same(&file_at(role,name,pin)?,&show)?||
       !super::same(&file_at(site,name,pin)?,&raw)?{return Err(no());}
    Ok(f)
}
fn true_eof(f:&File,at:u64)->io::Result<bool>{
    loop{match f.read_at(&mut [0],at){Ok(n)=>return Ok(n==0),Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,Err(e)=>return Err(e)}}
}
fn stable(before:&std::fs::Metadata,after:&std::fs::Metadata)->bool{
    (before.ctime(),before.ctime_nsec(),before.mtime(),before.mtime_nsec(),before.len())==
    (after.ctime(),after.ctime_nsec(),after.mtime(),after.mtime_nsec(),after.len())
}
fn exact<const N:usize>(f:&File)->io::Result<[u8;N]>{
    exact_public(f,N as u64)?;let m=f.metadata()?;let mut b=[0;N];f.read_exact_at(&mut b,0)?;
    let mut eof=[0];loop{match f.read_at(&mut eof,N as u64){Ok(0)=>break,Err(e) if e.kind()==io::ErrorKind::Interrupted=>{},_=>return Err(no())}}
    let after=f.metadata()?;exact_public(f,N as u64)?;
    if (m.ctime(),m.ctime_nsec(),m.mtime(),m.mtime_nsec())!=(after.ctime(),after.ctime_nsec(),after.mtime(),after.mtime_nsec()){return Err(no());}
    Ok(b)
}
// The offline installer takes SYSTEM then site. Use *independent* nonblocking
// locks on these same directory inodes before either service exists. Never
// convert the installer's OFD or lock one of SQLite's files.
#[cfg(test)]
struct RuntimeLease{_system:File,_site:File}
#[cfg(test)]
impl RuntimeLease{
    pub(super) fn acquire(system:&File,site:&File)->io::Result<Self>{
        if ident(system)?==ident(site)?{return Err(no());}
        let take=|pin:&File,op|->io::Result<File>{
            directory(pin,false)?;
            let d=file_at(pin,".",OFlags::RDONLY|OFlags::CLOEXEC|OFlags::DIRECTORY|OFlags::NOFOLLOW)?;
            if !super::same(&d,pin)?{return Err(no());}flock(&d,op)?;
            if !super::same(&d,pin)?||d.metadata()?.nlink()==0{return Err(no());}Ok(d)
        };
        let system=take(system,FlockOperation::NonBlockingLockShared)?;
        let site=take(site,FlockOperation::NonBlockingLockExclusive)?;Ok(Self{_system:system,_site:site})
    }
}
pub(super) struct Selected<'a>{pub(super) inputs:Inputs<'a>,pub(super) daemon:Image<'a>,pub(super) storage:Image<'a>,
    pub(super) env:StorageConfig,live:Vec<u8>}
impl<'a> Selected<'a>{
    pub(super) fn new(inputs:Inputs<'a>)->io::Result<Self>{
        if inputs.bound_sha==[0;32]{return Err(no());}
        let o=inputs.plan.origin;
        let live=smb_profile::render(inputs.plan).map_err(|_|no())?.into_bytes();
        let config:[u8;32]=Sha256::digest(&live).into();
        if (o.pool,o.instance,o.domain)!=(inputs.pool,inputs.instance,inputs.operation)||config!=inputs.live_sha{return Err(no());}
        let env=StorageConfig::selected(inputs.uid,inputs.gid,inputs.system_path,inputs.pool,inputs.instance,inputs.operation,config)?;
        let release=inputs.release;
        let daemon=Image::select(release,true)?;let storage=Image::select(release,false)?;
        let x=Self{inputs,daemon,storage,env,live};x.require(View::Prepared,View::Prepared)?;Ok(x)
    }
    pub(super) fn require(&self,daemon_view:View<'_>,storage_view:View<'_>)->io::Result<[u8;128]>{
        let a=&self.inputs;a.approved.require()?;self.daemon.original()?;self.storage.original()?;
        if a.system_pin.path()!=std::path::Path::new(a.system_path)||
            a.system_pin.entity()!=ident(a.system)?||a.system_pin.require_same_named_mount()?.id!=mid(a.system)?{
            return Err(no());
        }
        if ident(self.daemon.file)?==ident(self.storage.file)?{return Err(no());}
        let host=File::from(open(c"/",DIR,Mode::empty())?);
        daemon_view.require(a.daemon_root,&host,true)?;storage_view.require(a.storage_root,&host,false)?;
        super::identity::require(a.daemon_root,a.storage_root,a.approved_nss_sha,(a.uid,a.gid))?;
        let ids=[ident(a.daemon_root)?,ident(a.storage_root)?,ident(a.site)?,ident(a.system)?];
        if ids.iter().enumerate().any(|(i,k)|ids[..i].contains(k)){return Err(no());}
        let role=dir(a.daemon_root,"instance")?;directory(&role,true)?;directory(a.site,true)?;
        let site=a.site.metadata()?;let sys=a.system.metadata()?;
        if site.gid()!=a.gid||site.mode()&0o7777!=0o750||!sys.is_dir()||sys.uid()!=a.uid||
            sys.mode()&0o7777!=0o700{return Err(no());}
        for n in ["private","state"]{
            let orig=dir(a.site,n)?;let projected=dir(&role,n)?;let m=orig.metadata()?;
            if ident(&orig)?!=ident(&projected)?||mid(&orig)?!=mid(a.site)?||mid(&orig)?==mid(&projected)?||
                m.mode()&0o7777!=0o700||m.uid()!=a.uid||m.gid()!=a.gid||ro(&projected)?{return Err(no());}
        }
        if ident(&dir(a.site,"private")?)?==ident(&dir(a.site,"state")?)?{return Err(no());}
        // TDB has no presence in either Web or storage. The isolated
        // smbd sees one read-only directory projection of the actual host
        // inode; a same-byte copy or read-only file alone is not sufficient.
        let account=dir(a.site,"accountdb")?;let account_view=dir(&role,"accountdb")?;
        let m=account.metadata()?;
        if m.mode()!=0o040700||m.uid()!=0||m.gid()!=0||m.nlink()==0||mid(&account)?!=mid(a.site)?||
            ident(&account)?!=ident(&account_view)?||mid(&account)?==mid(&account_view)?||!ro(&account_view)?||
            ["private","state"].iter().any(|n|dir(a.site,n).and_then(|f|ident(&f)).ok()==ident(&account).ok()){
            return Err(no());
        }
        let db_path=OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC;
        let store=file_at(&account,"passdb.tdb",db_path)?;let shown=file_at(&account_view,"passdb.tdb",db_path)?;
        let f=store.metadata()?;
        if f.mode()!=0o100600||f.uid()!=0||f.gid()!=0||f.nlink()!=1||!(1..=64<<20).contains(&f.len())||
            mid(&store)?!=mid(&account)?||mid(&shown)?!=mid(&account_view)?||ident(&store)?!=ident(&shown)?||!ro(&shown)?{
            return Err(no());
        }
        if matches!(daemon_view,View::Prepared){
            for n in SCRATCH{
                let d=dir(&role,n)?;directory(&d,true)?;if mid(&d)?!=mid(&role)?||!ro(&d)?{return Err(no());}
            }
        }
        for (path,orig) in std::iter::once((std::path::Path::new(a.system_path),a.system)).chain(a.approved.roots()){
            let relative=path.strip_prefix("/").map_err(|_|no())?;
            let shown=File::from(openat2(a.storage_root,relative,DIR,Mode::empty(),SAFE)?);
            let on=orig.metadata()?;let m=shown.metadata()?;
            if ident(&shown)?!=ident(orig)?||mid(&shown)?==mid(orig)?||m.uid()!=on.uid()||m.gid()!=on.gid()||
                m.mode()!=on.mode()||ro(&shown)?!=ro(orig)?||Mount::on(&shown)?.is_none_or(|p|p.at_root!=Some(true)){
                return Err(no());
            }
            // The daemon receives no SYSTEM, F, or R source whatsoever. A similarly
            // spelled generated path is not a source or authorized alias.
            match openat2(a.daemon_root,relative,DIR,Mode::empty(),SAFE){
                Err(Errno::NOENT|Errno::NOTDIR)=>(),
                Ok(f)=>{if ident(&File::from(f))?==ident(orig)?{return Err(no());}}
                _=>return Err(no()),
            }
        }
        let live=projected(a.site,&role,"smb.conf",self.live.len() as u64)?;
        let mut exact_live=vec![0;self.live.len()];
        let before=live.metadata()?;
        live.read_exact_at(&mut exact_live,0)?;
        if exact_live!=self.live||!true_eof(&live,self.live.len() as u64)?||
            !stable(&before,&live.metadata()?)||!super::same(&live,&projected(a.site,&role,"smb.conf",self.live.len() as u64)?)?{
            return Err(no());
        }
        let service_bound=projected(a.site,&role,"domain.bound",128)?;
        // Do not read a single byte from a wrong caller-supplied descriptor,
        // even when it looks like a 128B, root-owned public record.
        exact_public(a.bound_readonly,128)?;let flags=fcntl_getfl(a.bound_readonly)?;
        if !ro(a.bound_readonly)?||flags.contains(OFlags::PATH)||flags.bits()&3!=0||
            ident(&service_bound)?!=ident(a.bound_readonly)?{return Err(no());}
        let b=exact::<128>(a.bound_readonly)?;
        if exact::<128>(&service_bound)?!=b||<[u8;32]>::from(Sha256::digest(b))!=a.bound_sha||
            &b[..16]!=b"ZXS-SAM/BOUND-1\0"||b[16..32]!=a.pool||b[32..48]!=a.instance||b[48..64]!=a.operation||
            b[64..96]!=a.live_sha||&b[96..104]!=b"ZXSLOCAL"||
            b[104..116]!=[1,4,0,0,0,0,0,5,21,0,0,0]||b[116..]==[0;12]{return Err(no());}
        Ok(b)
    }
}

#[cfg(test)]
mod lease_tests{
    use super::*;
    #[test]fn running_views_do_not_accept_readonly_scratch_or_an_arbitrary_proc_lookalike(){
        let proc=PROC_SUPER_MAGIC as u32;let tmp=0x0102_1994;
        let ro=StatVfsMountFlags::RDONLY;
        assert!(leaf_policy(Leaf::Proc,proc,0o040555,0,0,1,ISOLATED|ro));
        assert!(leaf_policy(Leaf::Scratch,tmp,0o040700,0,0,1,ISOLATED));
        assert!(!leaf_policy(Leaf::Proc,tmp,0o040555,0,0,1,ISOLATED|ro));
        assert!(!leaf_policy(Leaf::Proc,proc,0o040555,0,0,2,ISOLATED|ro));
        assert!(!leaf_policy(Leaf::Proc,proc,0o040555,0,0,1,ISOLATED));
        for (mode,owner,group,flags) in [(0o040755,0,0,ISOLATED),(0o040700,1000,0,ISOLATED),
            (0o040700,0,1000,ISOLATED),(0o040700,0,0,ISOLATED|ro),
            (0o040700,0,0,ISOLATED-StatVfsMountFlags::NOEXEC)]{
            assert!(!leaf_policy(Leaf::Scratch,tmp,mode,owner,group,1,flags));
        }
    }
    #[test]fn offline_admin_and_second_daemon_cannot_pass_while_online_pair_owns_the_originals(){
        let tmp=tempfile::tempdir().unwrap();let s=tmp.path().join("system");let i=tmp.path().join("instance");
        std::fs::create_dir(&s).unwrap();std::fs::create_dir(&i).unwrap();
        let(system,site)=(File::open(&s).unwrap(),File::open(&i).unwrap());
        let run=RuntimeLease::acquire(&system,&site).unwrap();
        let storage=File::open(&s).unwrap();flock(&storage,FlockOperation::NonBlockingLockShared).unwrap();
        assert_eq!(flock(&File::open(&s).unwrap(),FlockOperation::NonBlockingLockExclusive),Err(Errno::WOULDBLOCK));
        assert_eq!(flock(&File::open(&i).unwrap(),FlockOperation::NonBlockingLockExclusive),Err(Errno::WOULDBLOCK));
        assert!(RuntimeLease::acquire(&system,&site).is_err());
        drop(storage);drop(run);let offline=File::open(&s).unwrap();flock(&offline,FlockOperation::NonBlockingLockExclusive).unwrap();
        assert!(RuntimeLease::acquire(&system,&site).is_err());
        drop(offline);RuntimeLease::acquire(&system,&site).unwrap();
    }
}
