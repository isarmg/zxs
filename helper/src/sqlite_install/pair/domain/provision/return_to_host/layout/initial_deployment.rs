//! Creation-only host-side ZXS origin. Keep the fixed dedicated parent under
//! EX from the empty proof to selector fsync. An occupied name, abandoned
//! stage or loss of the original is never evidence of a successful issue.
use super::{attrs,bare_acl,creation_preflight,fstatvfs,host_mount,mark,new_dir,noentry,owned,publish,same_name,
    access_chain,protected_control,state_spelling,verify_registered_instance,INSTANCE,OWNER,PUBLIC,StatVfsMountFlags,Times};
use super::super::{Site,approval,bad,child,lease,public_record::Public,
    rust_site::{PublicAuthority,RustSite},site_lease::SiteLease};
use crate::{pinned::{Kind,Pinned,private_inode_reader},smb_profile::rust_first::MAX_WIRE,sqlite_install::{product_layout,pair::{
    fresh_issuer::{DEPLOYMENT_BYTES,FreshConsent,SitePayload,activation,require_v18},
    native_run::{FreshActive,ResolvedRust},terminal::PublicConfirmation}}};
use rustix::fs::{FlockOperation,Mode,OFlags,RawDir,ResolveFlags,flock,openat2};
use std::{ffi::{CStr,CString},fs::File,io::{self,Read},mem::MaybeUninit,os::unix::fs::FileExt,path::Path};

const ENTRY:&CStr=c"deployment.v1";
const TICKET:&CStr=c".deployment.v1.new";
const PLAN:&CStr=c"rust.install";
const SITE:&CStr=c"site.v1";
const VAULT:&CStr=c"vault";
const PARENT:&str=product_layout::STATE;
const PRIVATE:Mode=Mode::from_bits_retain(0o600);
fn no()->io::Error{bad("not a new protected standalone ZXS deployment; never reuse an earlier layout")}

fn inventory(dir:&File,expected:&[&[u8]])->io::Result<()>{
    let before=Times::on(dir)?;
    let other=File::from(rustix::fs::openat(dir,c".",OFlags::RDONLY|OFlags::DIRECTORY|OFlags::NONBLOCK|
        OFlags::CLOEXEC|OFlags::NOFOLLOW|OFlags::NOATIME,Mode::empty())?);
    if mark(dir)?!=mark(&other)?||expected.len()>3{return Err(no());}
    let mut buf=[MaybeUninit::uninit();4096];let mut raw=RawDir::new(&other,&mut buf);
    let mut seen=0u8;
    while let Some(e)=raw.next(){
        let e=e?;let n=e.file_name().to_bytes();
        if matches!(n,b"."|b".."){continue;}
        let i=expected.iter().position(|f|*f==n).ok_or_else(no)?;
        if seen&(1<<i)!=0{return Err(no());}
        seen|=1<<i;
    }
    if seen!=(1<<expected.len())-1||Times::on(dir)?!=before||Times::on(&other)?!=before{return Err(no());}
    Ok(())
}
fn flags()->ResolveFlags{
    ResolveFlags::BENEATH|ResolveFlags::NO_XDEV|ResolveFlags::NO_MAGICLINKS|ResolveFlags::NO_SYMLINKS
}
fn leaf(parent:&File,name:&CStr)->io::Result<File>{
    Ok(File::from(openat2(parent,name,OFlags::PATH|OFlags::CLOEXEC|OFlags::NOFOLLOW,Mode::empty(),flags())?))
}
fn new_file(parent:&File,name:&CStr,bytes:&[u8],mode:Mode)->io::Result<File>{
    let file=File::from(openat2(parent,name,OFlags::CREATE|OFlags::EXCL|OFlags::RDWR|OFlags::NOFOLLOW|
        OFlags::NONBLOCK|OFlags::CLOEXEC,PRIVATE,flags())?);
    same_name(parent,name,&file)?;attrs(&file,0,0,PRIVATE,false)?;
    bare_acl(&file,c"system.posix_acl_access")?;
    if file.metadata()?.len()!=0{return Err(no());}
    file.write_all_at(bytes,0)?;
    if mode!=PRIVATE{owned(&file,0,0,mode)?;}
    file.sync_all()?;Ok(file)
}
fn verify_file(parent:&File,name:&CStr,original:&File,wanted:&[u8],mode:Mode)->io::Result<()>{
    let pin=leaf(parent,name)?;
    same_name(parent,name,original)?;attrs(&pin,0,0,mode,false)?;
    let loc=mark(parent)?;let at=mark(&pin)?;
    if at.1!=loc.1||at.0.dev!=loc.0.dev||at!=mark(original)?||
        pin.metadata()?.len()!=wanted.len() as u64{return Err(no());}
    let stamp=Times::on(&pin)?;
    let read=if mode==PUBLIC{crate::pinned::public_record_reader(&pin,at.1,wanted.len() as u64)?}
        else{private_inode_reader(&pin,0,at.1)?};
    bare_acl(&read,c"system.posix_acl_access")?;
    let mut actual=Vec::new();(&read).take((wanted.len()+1)as u64).read_to_end(&mut actual)?;
    if actual!=wanted||Times::on(&read)?!=stamp||Times::on(original)?!=stamp
        ||mark(&leaf(parent,name)?)?!=at{return Err(no());}
    Ok(())
}

#[must_use]
pub(in crate::sqlite_install::pair) struct FreshParent{parent:Site,ex:File,stage:CString,instance:[u8;16]}
impl FreshParent{
    pub(in crate::sqlite_install::pair) fn select(instance:[u8;16])->io::Result<Self>{
        require_v18()?;
        approval::no_inherited_descriptors()?;
        approval::same_terminal(&io::stdin(),&io::stderr())?;
        if instance==[0;16]{return Err(no());}
        let parent=Site::product(approval::host_admin()?,&instance,true)?;
        if parent.chain.len()!=parent.prefix().len(){return Err(no());}
        let ex=child(parent.base(),c".")?;
        if mark(&ex)?!=mark(parent.base())?{return Err(no());}
        protected_control(&ex)?;
        flock(&ex,FlockOperation::NonBlockingLockExclusive)?;
        let leaf=parent.leaf.to_str().map_err(|_|no())?;
        let stage=CString::new(format!(".{leaf}.zxs-new")).map_err(|_|no())?;
        let p=Self{parent,ex,stage,instance};p.before_original_sql(&instance)?;Ok(p)
    }
    pub(in crate::sqlite_install::pair) fn before_original_sql(&self,instance:&[u8;16])->io::Result<()>{
        if instance!=&self.instance{return Err(no());}
        self.parent.check()?;host_mount(&self.parent)?;
        state_spelling(&self.parent.host,&self.parent.chain[0],&self.parent.chain[1])?;
        lease(&self.ex,mark(self.parent.base())?)?;
        protected_control(&self.ex)?;
        inventory(&self.ex,&[])
    }
    #[cfg(any(test,feature="embedded-zxs"))]
    pub(in crate::sqlite_install::pair) fn protect_image_destination(&self,target:crate::pinned::Entity)->io::Result<crate::pinned::Entity>{
        self.before_original_sql(&self.instance)?;
        for original in std::iter::once(&self.parent.host).chain(&self.parent.chain){
            if mark(original)?.0==target{return Err(no());}
        }
        Ok(mark(self.parent.base())?.0)
    }
    pub(in crate::sqlite_install::pair) fn product_root(&self)->io::Result<crate::pinned::Entity>{
        self.before_original_sql(&self.instance)?;Ok(mark(&self.parent.chain[0])?.0)
    }
    pub(in crate::sqlite_install::pair) fn require_distinct_image(&self,image:&crate::sqlite_install::pair::fresh_issuer::image::Image)
        ->io::Result<()>{
        self.before_original_sql(&self.instance)?;
        if image.includes(mark(self.parent.base())?.0){return Err(no());}
        for original in &self.parent.chain{if mark(original)?.0==image.entity(){return Err(no());}}
        image.require()
    }
    pub(in crate::sqlite_install::pair) fn bind(self,resolved:&ResolvedRust,authority:SitePayload)->io::Result<FreshHome>{
        if authority.origin!=resolved.plan().origin||authority.plan_sha!=resolved.plan().sha(){return Err(no());}
        self.before_original_sql(&authority.origin.instance)?;
        let home=FreshHome{parent:self.parent,ex:self.ex,stage:self.stage,authority};
        home.check(resolved,&[])?;Ok(home)
    }
}

#[must_use]
pub(in crate::sqlite_install::pair) struct FreshHome{
    parent:Site,ex:File,stage:CString,authority:SitePayload,
}
impl FreshHome{
    /// Report only the actually retained, no-clobber parent and original
    /// identity. The private setup route owns the administrator's approval.
    pub(in crate::sqlite_install::pair) fn evidence(&self)->io::Result<String>{
        let (entity,mount)=mark(&self.ex)?;
        Ok(format!("{PARENT}/{}; dedicated parent dev {} / inode {} / mount {mount}",
            self.parent.leaf.to_string_lossy(),entity.dev,entity.ino))
    }
    fn check(&self,resolved:&ResolvedRust,expected:&[&[u8]])->io::Result<()>{
        let (s,_,actor)=resolved.protected_new_system()?;
        if self.authority.system.as_path()!=s.path()||self.authority.subject!=actor||
            self.authority.origin!=resolved.plan().origin||self.authority.plan_sha!=resolved.plan().sha(){return Err(no());}
        resolved.require_distinct_site(Path::new(PARENT))?;
        access_chain(&self.parent,actor.tuple().2)?;
        host_mount(&self.parent)?;
        let mut chain=vec![mark(&self.parent.host)?.0];
        for p in &self.parent.chain{chain.push(mark(p)?.0);}
        resolved.require_product_parent(&chain)?;
        let id=mark(self.parent.base())?;
        lease(&self.ex,id)?;
        protected_control(&self.ex)?;
        creation_preflight(&self.ex)?;
        if usize::from(helper_linux_evidence::portable_component_limit(&self.ex)?)<self.stage.to_bytes().len()
            ||fstatvfs(&self.ex)?.f_flag.contains(StatVfsMountFlags::RDONLY){return Err(no());}
        inventory(&self.ex,expected)?;
        self.parent.check()
    }
    fn tree(&self,dir:&File,plan:&[u8],root:&[u8],vault:&File,plan_fd:&File,site_fd:&File)->io::Result<()>{
        inventory(dir,&[PLAN.to_bytes(),SITE.to_bytes(),VAULT.to_bytes()])?;
        let id=mark(dir)?;lease(dir,id)?;
        attrs(dir,0,self.authority.subject.tuple().2,INSTANCE,true)?;
        bare_acl(dir,c"system.posix_acl_access")?;bare_acl(dir,c"system.posix_acl_default")?;
        same_name(dir,VAULT,vault)?;attrs(vault,0,0,OWNER,true)?;
        bare_acl(vault,c"system.posix_acl_access")?;bare_acl(vault,c"system.posix_acl_default")?;inventory(vault,&[])?;
        verify_file(dir,PLAN,plan_fd,plan,PUBLIC)?;
        verify_file(dir,SITE,site_fd,root,PRIVATE)?;
        // Independently check the same parser/slot as the low first issuer.
        let pub_plan=Public::select(dir,PLAN,1..=MAX_WIRE)?;
        if pub_plan.bytes()!=plan||PublicAuthority::select(dir)?.bytes()!=root{return Err(no());}
        Ok(())
    }
    pub(in crate::sqlite_install::pair) fn prepare(&self,resolved:&ResolvedRust,consent:&FreshConsent,
        live:impl Fn()->io::Result<()>)->io::Result<RustSite>{
        let final_name=self.parent.leaf.as_c_str();
        let plan=resolved.plan().wire();
        let root=self.authority.encode()?;
        self.check(resolved,&[])?;consent.require_site(&self.authority)?;live()?;
        noentry(&self.ex,final_name)?;
        let created=new_dir(&self.ex,&self.stage)?;
        flock(&created,FlockOperation::NonBlockingLockExclusive)?;
        self.check(resolved,&[self.stage.to_bytes()])?;consent.require_site(&self.authority)?;live()?;
        let vault=new_dir(&created,VAULT)?;owned(&vault,0,0,OWNER)?;
        let plan_fd=new_file(&created,PLAN,plan,PUBLIC)?;
        let site_fd=new_file(&created,SITE,&root,PRIVATE)?;
        let gid=self.authority.subject.tuple().2;
        owned(&created,0,gid,INSTANCE)?;
        vault.sync_all()?;created.sync_all()?;self.ex.sync_all()?;
        self.check(resolved,&[self.stage.to_bytes()])?;
        self.tree(&created,plan,&root,&vault,&plan_fd,&site_fd)?;
        consent.require_site(&self.authority)?;live()?;
        same_name(&self.ex,&self.stage,&created)?;noentry(&self.ex,final_name)?;
        publish(&self.ex,&self.stage,final_name)?;
        same_name(&self.ex,final_name,&created)?;
        self.ex.sync_all()?;
        self.check(resolved,&[final_name.to_bytes()])?;
        self.tree(&created,plan,&root,&vault,&plan_fd,&site_fd)?;
        let site=Site::product(approval::host_admin()?,&self.authority.origin.instance,false)?;
        if mark(site.base())?!=mark(&created)?{return Err(no());}
        verify_registered_instance(&site,gid)?;
        // Reopen only the exact original public pathname; duplicate, never
        // reacquire or convert, the actual S EX and creator's site EX.
        let (old,system_lock,subject)=resolved.protected_new_system()?;
        let pin=Pinned::already_mounted(old.path(),Kind::StorageRoot)?;
        if pin.entity()!=old.entity()||pin.require_same_named_mount()?!=old.require_same_named_mount()?{return Err(no());}
        let held=SiteLease::from_product(pin,system_lock.try_clone()?,site,created,subject)?;
        consent.require_site(&self.authority)?;live()?;
        RustSite::selected_product(held,self.authority.clone(),resolved.plan())
    }

    /// Publication takes the nonconstructible proof of actual lowered ACTIVE.
    /// No child starts here; no old selector or partial stage is adopted.
    pub(in crate::sqlite_install::pair) fn activate(self,resolved:&ResolvedRust,site:&RustSite,active:FreshActive<'_>,
        tty:&PublicConfirmation,live:impl Fn()->io::Result<()>)->io::Result<[u8;32]>{
        let name=self.parent.leaf.as_c_str();
        let expected=activation(&self.authority)?;
        let check=||->io::Result<()>{
            tty.check()?;live()?;
            if site.fresh_payload()!=Some(&self.authority)||active.at(site)?.generation!=1{return Err(no());}
            Ok(())
        };
        self.check(resolved,&[name.to_bytes()])?;check()?;
        let ticket=new_file(&self.ex,TICKET,&expected,PRIVATE)?;
        self.ex.sync_all()?;
        self.check(resolved,&[name.to_bytes(),TICKET.to_bytes()])?;
        verify_file(&self.ex,TICKET,&ticket,&expected,PRIVATE)?;
        check()?;
        same_name(&self.ex,TICKET,&ticket)?;noentry(&self.ex,ENTRY)?;
        publish(&self.ex,TICKET,ENTRY)?;
        self.ex.sync_all()?;
        self.check(resolved,&[name.to_bytes(),ENTRY.to_bytes()])?;
        verify_file(&self.ex,ENTRY,&ticket,&expected,PRIVATE)?;
        check()?;
        // The two best-known filesystem names are held. No extra record or
        // secret crosses the terminal; cold start/login remain a later test.
        let digest=self.authority.sha()?;
        if expected.len()!=DEPLOYMENT_BYTES{return Err(no());}
        Ok(digest)
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn fresh_v18_exact_dedicated_inventory_and_noreplace_use_only_disposable_names(){
        let tmp=tempfile::tempdir().unwrap();let dir=File::open(tmp.path()).unwrap();
        inventory(&dir,&[]).unwrap();
        std::fs::write(tmp.path().join(".deployment.v1.new"),b"fresh-candidate").unwrap();
        assert!(inventory(&dir,&[]).is_err());
        inventory(&dir,&[TICKET.to_bytes()]).unwrap();
        std::fs::write(tmp.path().join("deployment.v1"),b"original-must-stay").unwrap();
        assert!(publish(&dir,TICKET,ENTRY).is_err());
        assert_eq!(std::fs::read(tmp.path().join("deployment.v1")).unwrap(),b"original-must-stay");
        assert_eq!(std::fs::read(tmp.path().join(".deployment.v1.new")).unwrap(),b"fresh-candidate");
        assert!(inventory(&dir,&[ENTRY.to_bytes()]).is_err());
    }
}
