//! The host side of a single offline vault placement. No password or
//! password-equivalent bytes are read here or given to the SQL worker.
use super::{bad,contents,layout,mark,rust_site::{checked_vault_stamp,CredentialSite,RustSite,VaultStamp}};
use crate::{pinned::{Entity,private_inode_reader},sqlite_install::pair::{
    native_run::{DurableRustPending,RustEnrollmentApproval},native_sql::rust_vault::ActiveRustMeta,Id}};
use rustix::fs::{Mode,OFlags,RenameFlags,ResolveFlags,openat2,renameat,renameat_with};
use std::{ffi::{CStr,CString},fs::File,io,os::unix::fs::{FileExt,MetadataExt}};

const FIXED:&CStr=c"credential.v1";
const LOCAL:ResolveFlags=ResolveFlags::BENEATH.union(ResolveFlags::NO_XDEV)
    .union(ResolveFlags::NO_SYMLINKS).union(ResolveFlags::NO_MAGICLINKS);
fn wrong()->io::Error{bad("wrong exact offline Rust SMB credential state; preserve or query the original operation")}
fn stage_name(op:&Id)->io::Result<CString>{
    use std::fmt::Write;
    if op==&[0;16]{return Err(wrong());}
    let mut n=String::from("pending-");
    for b in op{write!(&mut n,"{b:02x}").map_err(|_|wrong())?;}
    CString::new(n).map_err(|_|wrong())
}
fn pin(dir:&File,name:&CStr)->io::Result<File>{
    Ok(File::from(openat2(dir,name,OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty(),LOCAL)?))
}
fn inventory(dir:&File,active:bool,stage:Option<&CStr>)->io::Result<()>{
    let mut names=[&b""[..];2];let mut count=0;
    if active{names[0]=FIXED.to_bytes();count=1;}
    if let Some(p)=stage{names[count]=p.to_bytes();count+=1;}
    contents(dir,&names[..count]).map_err(|_|wrong())
}
fn exact_site(site:CredentialSite<'_>,m:&ActiveRustMeta)->bool{
    let o=site.plan().origin;
    (m.pool,m.init,m.setup,m.instance,m.bind)==(o.pool,o.init,o.setup,o.instance,o.bind)&&m.descriptor_sha==site.plan().sha()
}
fn successor(site:CredentialSite<'_>,old:Option<&ActiveRustMeta>,new:&ActiveRustMeta)->bool{
    if !exact_site(site,new){return false;}
    match old{
        None=>new.generation==1&&new.op==new.setup,
        Some(p)=>exact_site(site,p)&&p.generation>=1&&p.generation.checked_add(1)==Some(new.generation)&&
            p.account==new.account&&p.login==new.login&&p.sid==new.sid&&p.op!=new.op&&p.record!=new.record,
    }
}

struct Original<'s>{site:CredentialSite<'s>,dir:File,mark:(Entity,u64)}
impl<'s> Original<'s>{
    fn open(site:CredentialSite<'s>)->io::Result<Self>{
        let dir=site.offline_vault_directory()?;let mark=mark(&dir)?;
        let original=Self{site,dir,mark};original.require()?;Ok(original)
    }
    fn require(&self)->io::Result<()>{
        if mark(&self.dir)?!=self.mark||mark(&self.site.offline_vault_directory()?)?!=self.mark{return Err(wrong());}
        self.site.require()
    }
    fn exact(&self,name:&CStr,wanted:&[u8;224])->io::Result<Sealed>{
        self.require()?;
        let selected=pin(&self.dir,name)?;
        let stamp=checked_vault_stamp(&selected,self.mark.1)?;
        if stamp.dev!=self.mark.0.dev||stamp.ino==self.mark.0.ino{return Err(wrong());}
        let fd=private_inode_reader(&selected,0,self.mark.1)?;
        layout::bare_acl(&fd,c"system.posix_acl_access")?;
        let sealed=Sealed{fd,stamp};sealed.require(self,name,wanted)?;Ok(sealed)
    }
}

fn require_new_stage(file:&File,vault:(Entity,u64))->io::Result<()>{
    let m=file.metadata()?;let (entity,mount)=mark(file)?;
    if !m.is_file()||m.mode()!=0o100600||m.uid()!=0||m.gid()!=0||m.nlink()!=1||m.len()!=0||
        mount!=vault.1||entity.dev!=vault.0.dev||entity.ino==vault.0.ino{return Err(wrong());}
    Ok(())
}

struct Sealed{fd:File,stamp:VaultStamp}
impl Sealed{
    fn require(&self,dir:&Original<'_>,name:&CStr,wanted:&[u8;224])->io::Result<()>{
        let mount=dir.mark.1;
        if checked_vault_stamp(&self.fd,mount)?!=self.stamp||
            checked_vault_stamp(&pin(&dir.dir,name)?,mount)?!=self.stamp{return Err(wrong());}
        let mut prefix=[0;224];self.fd.read_exact_at(&mut prefix,0)?;
        if prefix!=*wanted||checked_vault_stamp(&self.fd,mount)?!=self.stamp||
            checked_vault_stamp(&pin(&dir.dir,name)?,mount)?!=self.stamp{return Err(wrong());}
        dir.require()
    }
    fn is_inode(&self,e:(Entity,u64),mount:u64)->bool{
        self.stamp.dev==e.0.dev&&self.stamp.ino==e.0.ino&&e.1==mount
    }
}
/// The same exact active vault member and its parent remain pinned across
/// terminal approval and the new stage. An occupied vault is never cleaned.
pub(in crate::sqlite_install::pair) struct CurrentVault<'s>{path:Original<'s>,meta:ActiveRustMeta,old:Sealed}
impl<'s> CurrentVault<'s>{
    pub(in crate::sqlite_install::pair) fn select(site:CredentialSite<'s>,meta:&ActiveRustMeta)->io::Result<Self>{
        if !matches!(site,CredentialSite::Deployed(_))||!exact_site(site,meta){return Err(wrong());}
        let path=Original::open(site)?;
        inventory(&path.dir,true,None)?;
        let old=path.exact(FIXED,&meta.record_prefix()?)?;
        let actual=Self{path,meta:meta.clone(),old};
        actual.require()?;Ok(actual)
    }
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{
        inventory(&self.path.dir,true,None)?;
        self.old.require(&self.path,FIXED,&self.meta.record_prefix()?)
    }
}

pub(in crate::sqlite_install::pair) struct Allocated<'s>{
    path:Original<'s>,name:CString,original:File,inode:(Entity,u64),meta:ActiveRustMeta,
    previous:Option<(ActiveRustMeta,Sealed)>,
}
impl<'s> Allocated<'s>{
    // Fresh only. The caller must close its writer description after lending
    // it; no Drop tries to unlink any entry on uncertain operation state.
    pub(in crate::sqlite_install::pair) fn reserve(grant:&RustEnrollmentApproval<'s>,current:Option<CurrentVault<'s>>,
        live:impl Fn()->io::Result<()>)->io::Result<(Self,File)>{
        grant.require()?;live()?;
        let site=grant.site();let target=grant.original().target.clone();let old=grant.original().previous.clone();
        if grant.original().state!=1||!successor(site,old.as_ref(),&target){return Err(wrong());}
        let (path,previous)=match (site,old,current){
            (CredentialSite::Fresh(_),None,None)=>(Original::open(site)?,None),
            (CredentialSite::Deployed(_),Some(m),Some(current))
                if site.same(current.path.site)&&m==current.meta=>{
                current.require()?;
                (current.path,Some((m,current.old)))
            },
            _=>return Err(wrong()),
        };
        let name=stage_name(&target.op)?;
        // No previously named stage can be used or deleted, even for this
        // exact Op. In a deployed reset only the qualified active old name
        // may be present. No execution path can rewrite an unknown record.
        layout::creation_preflight(&path.dir)?;
        inventory(&path.dir,previous.is_some(),None)?;
        if let Some((m,r))=&previous{r.require(&path,FIXED,&m.record_prefix()?)?;}
        grant.require()?;live()?;
        let fd=File::from(openat2(&path.dir,&name,OFlags::WRONLY|OFlags::CREATE|OFlags::EXCL|OFlags::NOFOLLOW|
            OFlags::CLOEXEC,Mode::RUSR|Mode::WUSR,LOCAL)?);
        let inode=mark(&fd)?;
        require_new_stage(&fd,path.mark)?;
        let original=pin(&path.dir,&name)?;
        if mark(&original)?!=inode{return Err(wrong());}
        layout::bare_acl(&fd,c"system.posix_acl_access")?;
        inventory(&path.dir,previous.is_some(),Some(&name))?;path.require()?;
        Ok((Self{path,name,original,inode,meta:target,previous},fd))
    }
    pub(in crate::sqlite_install::pair) fn reopen_after_reap(self)->io::Result<Frozen<'s>>{
        let wanted=self.meta.record_prefix()?;
        let sealed=self.path.exact(&self.name,&wanted)?;
        if !sealed.is_inode(self.inode,self.path.mark.1)||checked_vault_stamp(&self.original,self.path.mark.1)?!=sealed.stamp{
            return Err(wrong());
        }
        sealed.fd.sync_all()?;self.path.dir.sync_all()?;
        sealed.require(&self.path,&self.name,&wanted)?;
        inventory(&self.path.dir,self.previous.is_some(),Some(&self.name))?;
        let frozen=Frozen{allocated:self,sealed};
        frozen.require_ready()?;Ok(frozen)
    }
}

pub(in crate::sqlite_install::pair) struct Frozen<'s>{allocated:Allocated<'s>,sealed:Sealed}
impl<'s> Frozen<'s>{
    /// Last proof before the exact SQL worker is allowed to make the old
    /// epoch PENDING. Keep the original prior name, not merely its record ID.
    pub(in crate::sqlite_install::pair) fn require_ready(&self)->io::Result<()>{
        let a=&self.allocated;let path=&a.path;
        inventory(&path.dir,a.previous.is_some(),Some(&a.name))?;
        if let Some((old,original))=&a.previous{original.require(path,FIXED,&old.record_prefix()?)?;}
        self.sealed.require(path,&a.name,&a.meta.record_prefix()?)?;
        if checked_vault_stamp(&a.original,path.mark.1)?!=self.sealed.stamp{return Err(wrong());}
        Ok(())
    }
    pub(in crate::sqlite_install::pair) fn replace_in_pending(self,pending:&DurableRustPending<'_,'_>)->io::Result<PlacedVault<'s>>{
        let a=&self.allocated;let path=&a.path;
        pending.require(path.site,&a.meta)?;path.require()?;
        let wanted=a.meta.record_prefix()?;
        self.require_ready()?;
        pending.require(path.site,&a.meta)?;
        if a.previous.is_some(){
            renameat(&path.dir,&a.name,&path.dir,FIXED)?;
        }else{
            renameat_with(&path.dir,&a.name,&path.dir,FIXED,RenameFlags::NOREPLACE)?;
        }
        inventory(&path.dir,true,None)?;
        let installed=path.exact(FIXED,&wanted)?;
        if !installed.is_inode(a.inode,path.mark.1){return Err(wrong());}
        installed.fd.sync_all()?;path.dir.sync_all()?;
        let again=path.exact(FIXED,&wanted)?;
        if again.stamp!=installed.stamp||!again.is_inode(a.inode,path.mark.1)
            ||checked_vault_stamp(&a.original,path.mark.1)?!=again.stamp{return Err(wrong());}
        inventory(&path.dir,true,None)?;
        pending.require(path.site,&a.meta)?;
        let Allocated{path,meta,original,..}=self.allocated;
        let out=PlacedVault{path,meta,sealed:again,original};out.require()?;Ok(out)
    }
}
pub(in crate::sqlite_install::pair) struct PlacedVault<'s>{
    path:Original<'s>,meta:ActiveRustMeta,sealed:Sealed,original:File,
}
impl PlacedVault<'_>{
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{
        self.sealed.require(&self.path,FIXED,&self.meta.record_prefix()?)?;
        if checked_vault_stamp(&self.original,self.path.mark.1)?!=self.sealed.stamp{return Err(wrong());}
        inventory(&self.path.dir,true,None)
    }
    pub(in crate::sqlite_install::pair) fn wire(&self)->io::Result<([u8;65],&File)>{
        self.require()?;let mut b=[0;65];b[0]=3;b[1..17].copy_from_slice(&self.meta.record);
        let v=&self.sealed.stamp;
        b[17..25].copy_from_slice(&self.path.mark.1.to_be_bytes());
        b[25..33].copy_from_slice(&v.dev.to_be_bytes());b[33..41].copy_from_slice(&v.ino.to_be_bytes());
        b[41..49].copy_from_slice(&v.ctime_sec.to_be_bytes());b[49..53].copy_from_slice(&v.ctime_ns.to_be_bytes());
        b[53..61].copy_from_slice(&v.mtime_sec.to_be_bytes());b[61..65].copy_from_slice(&v.mtime_ns.to_be_bytes());
        Ok((b,&self.original))
    }
}

pub(in crate::sqlite_install::pair) fn verify_fresh_vault(site:&RustSite,target:&ActiveRustMeta)->io::Result<()>{
    if site.fresh_payload().is_none()||!successor(CredentialSite::Fresh(site),None,target){return Err(wrong());}
    let original=Original::open(CredentialSite::Fresh(site))?;
    inventory(&original.dir,true,None)?;
    let want=target.record_prefix()?;
    let held=original.exact(FIXED,&want)?;
    held.require(&original,FIXED,&want)?;
    inventory(&original.dir,true,None)?;
    original.require()
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn smb_password_reset_new_cannot_clear_or_adopt_an_unknown_stage_name(){
        // Exercise the real plain directory inventory only. These empty
        // placeholder files are neither a credential nor any actual vault.
        let shelf=tempfile::tempdir().unwrap();
        std::fs::File::create(shelf.path().join("credential.v1")).unwrap();
        let dir=std::fs::File::open(shelf.path()).unwrap();
        inventory(&dir,true,None).unwrap();
        let name=stage_name(&[50;16]).unwrap();
        let name=name.to_str().unwrap();
        std::fs::File::create(shelf.path().join(name)).unwrap();
        assert!(inventory(&dir,true,None).is_err());
        assert!(shelf.path().join(name).is_file()&&shelf.path().join("credential.v1").is_file());
    }
}
