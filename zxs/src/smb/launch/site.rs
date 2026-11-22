//! Fresh opt-in ZXS-owned runtime authority. Its approved public record
//! neither creates a site nor substitutes for the lowered ACTIVE SQLite
//! cut. No Helper path, policy, service or cached discovery is an input.
mod approval;
mod deployment;
mod held;
mod image;
mod layout;
mod terminal;
pub(super) mod media;
mod vault;
use super::custody::{Parent,VerifiedPreflight};
use approval::{Approval,MAX_PLAN,MAX_RECORD};
use deployment::Deployment;
use held::{PathPin,Place,PublicLeaf,Role};
pub(super) use approval::{Origin,Subject};
pub(super) use terminal::{StartApproval,Terminal};
use std::{fs::File,io,os::unix::fs::MetadataExt,path::{Path,PathBuf}};

const MEMBERS:&[&[u8]]=&[b"rust.install",b"site.v1",b"vault"];
fn denied()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"the protected original ZXS SMB site cannot be confirmed")}

#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) struct Binding{
    pub(super) authority:[u8;32],
    pub(super) system:Place,
    pub(super) site:Place,
}
/// Lifetime custody, not a credential, projected namespace or serving
/// grant. Host takes the original SYSTEM shared hold before the original
/// site's exclusive hold. It never opens the DB or changes directory bytes.
#[must_use]
pub(super) struct SiteLease<'p>{
    parent:&'p Parent,
    task:File,
    deployment:Deployment,
    system:PathPin,
    site:PathPin,
    image_pin:PathPin,
    image:image::Image,
    system_shared:File,
    site_exclusive:File,
    vault:File,
    record:PublicLeaf,
    plan:PublicLeaf,
    approval:Approval,
    binding:Binding,
}
fn instance_path(instance:[u8;16])->io::Result<PathBuf>{
    if instance==[0;16]{return Err(denied());}
    Ok(Path::new(layout::STATE).join(hex(&instance)))
}
fn protected_dir(dir:&File,gid:u32,mode:u32)->io::Result<()>{
    let m=dir.metadata()?;
    if !m.is_dir()||m.uid()!=0||m.gid()!=gid||m.mode()!=mode||m.nlink()==0{return Err(denied());}
    held::no_xattr(dir,c"system.posix_acl_access")?;
    held::no_xattr(dir,c"system.posix_acl_default")
}
impl<'p> SiteLease<'p>{
    pub(super) fn select(parent:&'p Parent)->io::Result<Self>{
        let host=parent.host().map_err(|_|denied())?;
        let task=parent.own_task().map_err(|_|denied())?;
        let deployment=Deployment::select(host,&task).map_err(|e|io::Error::new(e.kind(),format!(
            "cannot select the protected {}/deployment.v1: {e}; check the fresh offline setup; no layout was created or migrated",layout::STATE)))?;
        let instance=deployment.instance();
        let name=instance_path(instance)?;
        let site=PathPin::select(host,&task,&name,Role::FixedDir)?;
        let site_exclusive=held::dir_reader(site.held())?;
        // Read only a qualified root-owned record before selecting the S
        // it names, holding the site's exact inode but not yet its lease.
        // The owner and directory shape are independently checked now.
        let starter=site_exclusive.metadata()?;
        protected_dir(&site_exclusive,starter.gid(),0o040750)?;
        held::exactly(&site_exclusive,MEMBERS)?;
        let record=PublicLeaf::select(&task,&site_exclusive,c"site.v1",0o100600,MAX_RECORD)?;
        let authority=crate::hash::digest(record.bytes())?;
        deployment.require(host,&task,authority)?;
        let approval=Approval::decode(record.bytes())?;
        if approval.origin.instance!=instance||starter.gid()!=approval.subject.gid{return Err(denied());}
        let system=PathPin::select(host,&task,&approval.system,Role::System(approval.subject.uid,deployment.product()))?;
        let image_pin=PathPin::select(host,&task,&approval.image,Role::Image(deployment.product()))?;
        if deployment.overlaps(&system)||deployment.overlaps(&image_pin)||
            site.overlaps(&system)||image_pin.overlaps(&system)||image_pin.overlaps(&site){return Err(denied());}
        let system_shared=held::dir_reader(system.held())?;
        system_shared.try_lock_shared().map_err(io::Error::from)?;
        site_exclusive.try_lock().map_err(io::Error::from)?;
        site.require(host,&task)?;system.require(host,&task)?;image_pin.require(host,&task)?;
        record.require(&task,&site_exclusive)?;
        let plan=PublicLeaf::select(&task,&site_exclusive,c"rust.install",0o100444,MAX_PLAN)?;
        approval.require_plan(plan.bytes())?;
        let vault=held::dir_reader(&held::leaf(&site_exclusive,c"vault")?)?;
        protected_dir(&vault,0,0o040700)?;
        if held::place(&vault)?.mount!=site.location().mount{return Err(denied());}
        let image=image::Image::select(parent,&task,&image_pin,approval.image_sha)?;
        let binding=Binding{authority,system:system.location(),site:site.location()};
        let s=Self{parent,task,deployment,system,site,image_pin,image,system_shared,site_exclusive,vault,record,plan,approval,binding};
        s.require()?;Ok(s)
    }
    pub(super) fn require(&self)->io::Result<()>{
        let host=self.parent.host().map_err(|_|denied())?;
        self.deployment.require(host,&self.task,self.binding.authority)?;
        self.site.require(host,&self.task)?;self.system.require(host,&self.task)?;
        if held::place(&self.system_shared)?!=self.binding.system||held::place(&self.site_exclusive)?!=self.binding.site{
            return Err(denied());
        }
        // Check only the dedicated site's exact contents; the fixed host
        // ancestor spelling checks have a bounded no-atime name lookup.
        // Never list ROOT data or read contents of unrelated name entries.
        protected_dir(&self.site_exclusive,self.approval.subject.gid,0o040750)?;
        held::exactly(&self.site_exclusive,MEMBERS)?;
        self.record.require(&self.task,&self.site_exclusive)?;
        self.plan.require(&self.task,&self.site_exclusive)?;
        if held::place(&self.vault)?!=held::place(&held::leaf(&self.site_exclusive,c"vault")?)?{
            return Err(denied());
        }
        protected_dir(&self.vault,0,0o040700)?;
        self.image.require(self.parent,&self.task,&self.image_pin)?;
        self.site.require(host,&self.task)?;self.system.require(host,&self.task)?;
        self.deployment.require(host,&self.task,self.binding.authority)
    }
    pub(super) fn is_original_parent(&self,parent:&Parent)->bool{std::ptr::eq(self.parent,parent)}
    pub(super) fn binding(&self)->Binding{self.binding}
    pub(super) fn origin(&self)->Origin{self.approval.origin}
    pub(super) fn subject(&self)->Subject{self.approval.subject}
    pub(super) fn plan_bytes(&self)->&[u8]{self.plan.bytes()}
    pub(super) fn install_sha(&self)->[u8;32]{self.approval.plan_sha}
    pub(super) fn image_sha(&self)->[u8;32]{self.approval.image_sha}
    pub(super) fn image_path(&self)->&Path{&self.approval.image}
    pub(super) fn image(&self)->io::Result<&File>{self.require()?;Ok(self.image.file())}
    pub(super) fn system(&self)->io::Result<(&Path,&File,Place)>{
        self.require()?;Ok((&self.approval.system,&self.system_shared,self.binding.system))
    }
    /// An exact authenticated low ACTIVE proof must precede even opening the
    /// credential name. Pair the same proof with the original storage and
    /// site locks and never read any credential bytes as host root.
    pub(super) fn vault<'s>(&'s self,selected:&VerifiedPreflight<'s,'p>)->io::Result<vault::Vault<'s,'p>>{
        selected.qualified_for(self)?;
        let loan=vault::Vault::select(self)?;
        selected.qualified_for(self)?;
        loan.require()?;Ok(loan)
    }
}
fn hex(bytes:&[u8])->String{
    use std::fmt::Write as _;
    let mut out=String::with_capacity(2*bytes.len());
    for &b in bytes{write!(out,"{b:02x}").expect("format an in-memory String");}
    out
}
