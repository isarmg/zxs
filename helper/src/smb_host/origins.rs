//! Live, independently selected installation objects for one approved start.
//! This is not an authority issuer. No "trust these hashes" CLI or callback.
use super::{Image,STORAGE,no};
use crate::pinned::{Entity,Mount};
use helper_linux_evidence::smb_spawn::StorageConfig;
use rustix::{fs::{FlockOperation,Mode,OFlags,ResolveFlags,StatVfsMountFlags,fcntl_getfl,flock,fstatvfs,open,openat2},io::Errno};
use sha2::{Digest,Sha256};
use std::{fs::File,io,os::unix::fs::{FileExt,MetadataExt}};

const SAFE:ResolveFlags=ResolveFlags::BENEATH.union(ResolveFlags::NO_SYMLINKS).union(ResolveFlags::NO_MAGICLINKS);
const DIR:OFlags=OFlags::PATH.union(OFlags::DIRECTORY).union(OFlags::CLOEXEC).union(OFlags::NOFOLLOW);
const LIVE:&[u8]=include_bytes!("../../resources/samba-domain/initial-live.conf");

/// All fields are held originals from a separately authorized online installer,
/// not descriptive strings supplied by a web client or by the daemon. In
/// particular the prepared roots include trusted dynamic loaders/ELF closure;
/// the ABI here independently rechecks their relevant runtime projections.
/// There is deliberately no production issuer in the CLI or preflight tool.
pub(super) struct Inputs<'a>{
    pub(super) daemon_root:&'a File,pub(super) storage_root:&'a File,
    pub(super) site:&'a File,pub(super) system:&'a File,
    pub(super) bound_readonly:&'a File,pub(super) system_path:&'a str,
    pub(super) uid:u32,pub(super) gid:u32,pub(super) pool:[u8;16],pub(super) instance:[u8;16],
    pub(super) operation:[u8;16],pub(super) bound_sha:[u8;32],
    pub(super) smbd_sha:[u8;32],pub(super) storage_sha:[u8;32],
    pub(super) approved_nss_sha:[u8;32],pub(super) web_uid:u32,pub(super) web_gid:u32,
}
pub(super) fn file_at(f:&File,path:&str,flags:OFlags)->io::Result<File>{Ok(File::from(openat2(f,path,flags,Mode::empty(),SAFE)?))}
fn dir(f:&File,p:&str)->io::Result<File>{file_at(f,p,DIR)}
pub(super) fn ident(f:&File)->io::Result<Entity>{Entity::of(&f.metadata()?)}
pub(super) fn mid(f:&File)->io::Result<u64>{Ok(Mount::on(f)?.ok_or_else(no)?.id)}
pub(super) fn ro(f:&File)->io::Result<bool>{Ok(fstatvfs(f)?.f_flag.contains(StatVfsMountFlags::RDONLY))}
fn directory(f:&File,unowned:bool)->io::Result<()>{
    let m=f.metadata()?;if !m.is_dir() ||unowned&&(m.uid()!=0||m.mode()&0o022!=0){return Err(no());}Ok(())
}
fn view(f:&File,host:&File)->io::Result<()>{
    directory(f,true)?;
    if !ro(f)?||ident(f)?==ident(host)?||Mount::on(f)?.is_none_or(|m|m.at_root!=Some(true)){return Err(no());}
    let leaf=dir(f,"proc")?;directory(&leaf,true)?;if mid(&leaf)?!=mid(f)?{return Err(no());}Ok(())
}
fn exact_public(f:&File,len:u64)->io::Result<()>{
    let m=f.metadata()?;
    if !m.is_file()||m.uid()!=0||m.gid()!=0||m.nlink()!=1||m.len()!=len||m.mode()&0o7777!=0o444{return Err(no());}Ok(())
}
fn projected(site:&File,role:&File,name:&str,len:u64)->io::Result<File>{
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
pub(super) struct RuntimeLease{system:File,site:File}
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
        let site=take(site,FlockOperation::NonBlockingLockExclusive)?;Ok(Self{system,site})
    }
    pub(super) fn require(&self,a:&Inputs<'_>)->io::Result<()>{
        if !super::same(&self.system,a.system)?||!super::same(&self.site,a.site)?||
            self.system.metadata()?.nlink()==0||self.site.metadata()?.nlink()==0{return Err(no());}Ok(())
    }
}
pub(super) struct Selected<'a>{pub(super) inputs:Inputs<'a>,pub(super) daemon:Image,pub(super) storage:Image,pub(super) env:StorageConfig}
impl<'a> Selected<'a>{
    pub(super) fn new(inputs:Inputs<'a>)->io::Result<Self>{
        if inputs.smbd_sha==inputs.storage_sha||inputs.bound_sha==[0;32]{return Err(no());}
        let config:[u8;32]=Sha256::digest(LIVE).into();
        let env=StorageConfig::selected(inputs.uid,inputs.gid,inputs.system_path,inputs.pool,inputs.instance,inputs.operation,config)?;
        let daemon=Image::selected(inputs.smbd_sha)?;let storage=Image::at(STORAGE,inputs.storage_sha)?;
        let x=Self{inputs,daemon,storage,env};x.require()?;Ok(x)
    }
    pub(super) fn require(&self)->io::Result<()>{
        let a=&self.inputs;self.daemon.original()?;self.storage.original()?;
        if ident(&self.daemon.0.file)?==ident(&self.storage.0.file)?{return Err(no());}
        let host=File::from(open(c"/",DIR,Mode::empty())?);
        view(a.daemon_root,&host)?;view(a.storage_root,&host)?;
        super::identity::require(a.daemon_root,a.storage_root,a.approved_nss_sha,(a.uid,a.gid),(a.web_uid,a.web_gid))?;
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
        for n in ["run","cache","lock"]{
            let d=dir(&role,n)?;directory(&d,true)?;if mid(&d)?!=mid(&role)?||!ro(&d)?{return Err(no());}
        }
        let path=&a.system_path[1..];
        let s=dir(a.storage_root,path)?;
        if ident(&s)?!=ident(a.system)?||mid(&s)?==mid(a.system)?{return Err(no());}
        // Neither fresh role is allowed the other role's private input path.
        for(v,p)in [(a.storage_root,"instance"),(a.daemon_root,path)]{
            match openat2(v,p,OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty(),SAFE){Err(Errno::NOENT)=>{},_=>return Err(no())}
        }
        let live=projected(a.site,&role,"smb.conf",LIVE.len() as u64)?;
        if exact::<{LIVE.len()}>(&live)?.as_slice()!=LIVE{return Err(no());}
        let service_bound=projected(a.site,&role,"domain.bound",128)?;
        // Do not read a single byte from a wrong caller-supplied descriptor,
        // even when it looks like a 128B, root-owned public record.
        exact_public(a.bound_readonly,128)?;let flags=fcntl_getfl(a.bound_readonly)?;
        if !ro(a.bound_readonly)?||flags.contains(OFlags::PATH)||flags.bits()&3!=0||
            ident(&service_bound)?!=ident(a.bound_readonly)?{return Err(no());}
        let b=exact::<128>(a.bound_readonly)?;
        if exact::<128>(&service_bound)?!=b||<[u8;32]>::from(Sha256::digest(b))!=a.bound_sha||
            &b[..16]!=b"ZXS-SAM/BOUND-1\0"||b[16..32]!=a.pool||b[32..48]!=a.instance||b[48..64]!=a.operation||
            b[64..96]!=Sha256::digest(LIVE)[..]||&b[96..104]!=b"ZXSLOCAL"||
            b[104..116]!=[1,4,0,0,0,0,0,5,21,0,0,0]||b[116..]==[0;12]{return Err(no());}
        Ok(())
    }
}

#[cfg(test)]
mod lease_tests{
    use super::*;
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
