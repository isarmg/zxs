//! The host side of a single offline vault placement. No password or
//! password-equivalent bytes are read here or given to the SQL worker.
use super::{bad,contents,layout,mark,rust_site::{checked_vault_stamp,RustSite,VaultStamp}};
use crate::{pinned::{Entity,private_inode_reader},sqlite_install::pair::{
    native_run::{DurableRustPending,RustEnrollmentApproval},native_sql::rust_vault::ActiveRustMeta,Id}};
use rustix::fs::{AtFlags,Mode,OFlags,RenameFlags,openat,renameat,renameat_with,unlinkat};
use std::{ffi::{CStr,CString},fs::File,io,os::unix::fs::{FileExt,MetadataExt}};

const FIXED:&CStr=c"credential.v1";
fn wrong()->io::Error{bad("wrong exact offline Rust SMB credential state; preserve or query the original operation")}
fn stage_name(op:&Id)->io::Result<CString>{
    use std::fmt::Write;
    if op==&[0;16]{return Err(wrong());}
    let mut n=String::from("pending-");
    for b in op{write!(&mut n,"{b:02x}").map_err(|_|wrong())?;}
    CString::new(n).map_err(|_|wrong())
}
fn pin(dir:&File,name:&CStr)->io::Result<File>{
    Ok(File::from(openat(dir,name,OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?))
}
fn inventory(dir:&File,active:bool,stage:Option<&CStr>)->io::Result<()>{
    let mut names=[&b""[..];2];let mut count=0;
    if active{names[0]=FIXED.to_bytes();count=1;}
    if let Some(p)=stage{names[count]=p.to_bytes();count+=1;}
    contents(dir,&names[..count]).map_err(|_|wrong())
}
fn exact_site(site:&RustSite,m:&ActiveRustMeta)->bool{
    let o=site.plan().origin;
    (m.pool,m.init,m.setup,m.instance,m.bind)==(o.pool,o.init,o.setup,o.instance,o.bind)&&m.descriptor_sha==site.plan().sha()
}
fn successor(site:&RustSite,old:Option<&ActiveRustMeta>,new:&ActiveRustMeta)->bool{
    if !exact_site(site,new){return false;}
    match old{
        None=>new.generation==1&&new.op==new.setup,
        Some(p)=>exact_site(site,p)&&p.generation>=1&&p.generation.checked_add(1)==Some(new.generation)&&
            p.account==new.account&&p.login==new.login&&p.sid==new.sid&&p.op!=new.op&&p.record!=new.record,
    }
}

struct Original<'s>{site:&'s RustSite,dir:File,mark:(Entity,u64)}
impl<'s> Original<'s>{
    fn open(site:&'s RustSite)->io::Result<Self>{
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
    fn clear_unreserved(&self,grant:&RustEnrollmentApproval<'s>,name:&CStr,active:bool,
        live:&impl Fn()->io::Result<()>)->io::Result<()>{
        self.require()?;live()?;
        if grant.original().state!=1||!std::ptr::eq(self.site,grant.site())||
            stage_name(&grant.original().target.op)?.as_c_str()!=name{return Err(wrong());}
        let found=match pin(&self.dir,name){
            Err(e) if e.kind()==io::ErrorKind::NotFound=>return inventory(&self.dir,active,None),
            Err(e)=>return Err(e),Ok(found)=>found,
        };
        inventory(&self.dir,active,Some(name))?;
        let first=Unreserved::stamp(&found,self.mark)?;
        let opened=private_inode_reader(&found,0,self.mark.1)?;
        if Unreserved::stamp(&opened,self.mark)?!=first{return Err(wrong());}
        layout::bare_acl(&opened,c"system.posix_acl_access")?;
        let (system,_)=self.site.original_system()?;
        let instance=self.site.plan().origin.instance.iter().map(|x|format!("{x:02x}")).collect::<String>();
        let stage=name.to_str().map_err(|_|wrong())?;
        let review=format!("另行批准永久删除此未登记物。文件名字不证明它来自此前的写入。只在原 SQL 查无同一操作时可做。\n\
            不修改活动正本或 SQL。SYSTEM {:?}，站点 {}，固定 vault 项 {}。\n\
            原设备 {} / inode {} / mount {} / 长度 {} B；ctime {}.{:09} / mtime {}.{:09}。\n",
            system.path(),instance,stage,first.entity.dev,first.entity.ino,self.mark.1,first.len,
            first.ctime.0,first.ctime.1,first.mtime.0,first.mtime.1);
        live()?;grant.confirm_unregistered_stage(review)?;
        live()?;grant.require()?;self.require()?;
        inventory(&self.dir,active,Some(name))?;
        if Unreserved::stamp(&found,self.mark)?!=first||Unreserved::stamp(&opened,self.mark)?!=first||
            Unreserved::stamp(&pin(&self.dir,name)?,self.mark)?!=first{return Err(wrong());}
        layout::bare_acl(&opened,c"system.posix_acl_access")?;
        unlinkat(&self.dir,name,AtFlags::empty())?;
        self.dir.sync_all()?;
        inventory(&self.dir,active,None)
    }
}
#[derive(Clone,Copy,Eq,PartialEq)]
struct Unreserved{entity:Entity,len:u64,ctime:(i64,i64),mtime:(i64,i64)}
impl Unreserved{
    fn stamp(file:&File,vault:(Entity,u64))->io::Result<Self>{
        let m=file.metadata()?;let (entity,mount)=mark(file)?;
        if !m.is_file()||m.mode()!=0o100600||m.uid()!=0||m.gid()!=0||m.nlink()!=1||m.len()>240||
            mount!=vault.1||entity.dev!=vault.0.dev||entity.ino==vault.0.ino{return Err(wrong());}
        Ok(Self{entity,len:m.len(),ctime:(m.ctime(),m.ctime_nsec()),mtime:(m.mtime(),m.mtime_nsec())})
    }
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
pub(in crate::sqlite_install::pair) struct Allocated<'s>{
    path:Original<'s>,name:CString,inode:(Entity,u64),meta:ActiveRustMeta,
    previous:Option<(ActiveRustMeta,Sealed)>,
}
impl<'s> Allocated<'s>{
    // Fresh only. The caller must close its writer description after lending
    // it; no Drop tries to unlink any entry on uncertain operation state.
    pub(in crate::sqlite_install::pair) fn reserve(grant:&RustEnrollmentApproval<'s>,live:impl Fn()->io::Result<()>)
        ->io::Result<(Self,File)>{
        grant.require()?;live()?;
        let site=grant.site();let target=grant.original().target.clone();let old=grant.original().previous.clone();
        if grant.original().state!=1||!successor(site,old.as_ref(),&target){return Err(wrong());}
        let path=Original::open(site)?;
        let name=stage_name(&target.op)?;
        let previous=old.map(|m|{
            let held=path.exact(FIXED,&m.record_prefix()?)?;
            Ok::<_,io::Error>((m,held))
        }).transpose()?;
        if site.fresh_payload().is_some(){
            if previous.is_some(){return Err(wrong());}
            // A newly issued site never discards or resumes an earlier name.
            inventory(&path.dir,false,None)?;
        }else{path.clear_unreserved(grant,&name,previous.is_some(),&live)?;}
        grant.require()?;live()?;
        let fd=File::from(openat(&path.dir,&name,OFlags::WRONLY|OFlags::CREATE|OFlags::EXCL|OFlags::NOFOLLOW|
            OFlags::CLOEXEC,Mode::RUSR|Mode::WUSR)?);
        let inode=mark(&fd)?;
        let birth=Unreserved::stamp(&fd,path.mark)?;
        if birth.len!=0||mark(&pin(&path.dir,&name)?)?!=inode{return Err(wrong());}
        layout::bare_acl(&fd,c"system.posix_acl_access")?;
        inventory(&path.dir,previous.is_some(),Some(&name))?;path.require()?;
        Ok((Self{path,name,inode,meta:target,previous},fd))
    }
    pub(in crate::sqlite_install::pair) fn reopen_after_reap(self)->io::Result<Frozen<'s>>{
        let wanted=self.meta.record_prefix()?;
        let sealed=self.path.exact(&self.name,&wanted)?;
        if !sealed.is_inode(self.inode,self.path.mark.1){return Err(wrong());}
        sealed.fd.sync_all()?;self.path.dir.sync_all()?;
        sealed.require(&self.path,&self.name,&wanted)?;
        inventory(&self.path.dir,self.previous.is_some(),Some(&self.name))?;
        Ok(Frozen{allocated:self,sealed})
    }
}

pub(in crate::sqlite_install::pair) struct Frozen<'s>{allocated:Allocated<'s>,sealed:Sealed}
impl<'s> Frozen<'s>{
    pub(in crate::sqlite_install::pair) fn replace_in_pending(self,pending:&DurableRustPending<'_,'_>)->io::Result<PlacedVault<'s>>{
        let a=&self.allocated;let path=&a.path;
        pending.require(path.site,&a.meta)?;path.require()?;
        let wanted=a.meta.record_prefix()?;
        inventory(&path.dir,a.previous.is_some(),Some(&a.name))?;
        self.sealed.require(path,&a.name,&wanted)?;
        if let Some((old,original))=&a.previous{
            original.require(path,FIXED,&old.record_prefix()?)?;
            renameat(&path.dir,&a.name,&path.dir,FIXED)?;
        }else{
            renameat_with(&path.dir,&a.name,&path.dir,FIXED,RenameFlags::NOREPLACE)?;
        }
        inventory(&path.dir,true,None)?;
        let installed=path.exact(FIXED,&wanted)?;
        if !installed.is_inode(a.inode,path.mark.1){return Err(wrong());}
        installed.fd.sync_all()?;path.dir.sync_all()?;
        let again=path.exact(FIXED,&wanted)?;
        if again.stamp!=installed.stamp||!again.is_inode(a.inode,path.mark.1){return Err(wrong());}
        inventory(&path.dir,true,None)?;
        let original=pin(&path.dir,FIXED)?;
        pending.require(path.site,&a.meta)?;
        let Allocated{path,meta,..}=self.allocated;
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

// Only a truly existing and independently re-opened SQL PENDING may be
// resumed. Never prompt for new material, delete unknowns or fall back.
pub(in crate::sqlite_install::pair) fn finish_seen_pending<'s>(pending:&DurableRustPending<'_,'s>)
    ->io::Result<PlacedVault<'s>>{
    let grant=pending.approved();let site=grant.site();
    let old=grant.original().previous.clone();let target=grant.original().target.clone();
    pending.require(site,&target)?;
    if grant.original().state!=2||!successor(site,old.as_ref(),&target){return Err(wrong());}
    let path=Original::open(site)?;
    let name=stage_name(&target.op)?;
    let wanted=target.record_prefix()?;
    match pin(&path.dir,&name){
        Ok(found)=>{
            inventory(&path.dir,old.is_some(),Some(&name))?;
            let inode=mark(&found)?;let sealed=path.exact(&name,&wanted)?;
            if !sealed.is_inode(inode,path.mark.1){return Err(wrong());}
            let previous=old.map(|meta|{
                let old=path.exact(FIXED,&meta.record_prefix()?)?;
                Ok::<_,io::Error>((meta,old))
            }).transpose()?;
            sealed.fd.sync_all()?;path.dir.sync_all()?;
            Frozen{allocated:Allocated{path,name,inode,meta:target,previous},sealed}.replace_in_pending(pending)
        },
        Err(e) if e.kind()==io::ErrorKind::NotFound=>{
            inventory(&path.dir,true,None)?;
            let sealed=path.exact(FIXED,&wanted)?;
            sealed.fd.sync_all()?;path.dir.sync_all()?;
            sealed.require(&path,FIXED,&wanted)?;
            let original=pin(&path.dir,FIXED)?;pending.require(site,&target)?;
            let out=PlacedVault{path,meta:target,sealed,original};out.require()?;Ok(out)
        },
        Err(e)=>Err(e),
    }
}


pub(in crate::sqlite_install::pair) fn verify_fresh_vault(site:&RustSite,target:&ActiveRustMeta)->io::Result<()>{
    if site.fresh_payload().is_none()||!successor(site,None,target){return Err(wrong());}
    let original=Original::open(site)?;
    inventory(&original.dir,true,None)?;
    let want=target.record_prefix()?;
    let held=original.exact(FIXED,&want)?;
    held.require(&original,FIXED,&want)?;
    inventory(&original.dir,true,None)?;
    original.require()
}
