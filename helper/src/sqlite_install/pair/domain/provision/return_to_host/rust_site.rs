//! The original protected product site, no runtime or alternate account store.
use super::{DeployedSite,Site,approval,bad,child,contents,layout,mark,public_record::Public,site_lease::SiteLease};
use crate::{pinned::{Pinned,private_inode_reader},sqlite_install::pair::fresh_issuer::{CredentialScope,MAX_SITE,SitePayload,require_v18},
    smb_profile::rust_first::{MAX_WIRE,RustPlan}};
use rustix::fs::{Mode,OFlags,ResolveFlags,openat2};
use std::{fs::File,io::{self,Read},os::{unix::fs::MetadataExt},path::Path};

const PRODUCT:&[&[u8]]=&[b"rust.install",b"site.v1",b"vault"];
const AUTHORITY:&std::ffi::CStr=c"site.v1";
fn no()->io::Error{bad("not the protected original Rust SMB installation for this request")}

#[must_use]
pub(in crate::sqlite_install::pair) struct RustSite{
    held:SiteLease,installed:Public,plan:RustPlan,vault:File,
    authority:Option<(PublicAuthority,SitePayload)>,
}
impl RustSite{
    pub(super) fn selected_product(held:SiteLease,payload:SitePayload,expected:&RustPlan)->io::Result<Self>{
        require_v18()?;
        if held.subject!=payload.subject.tuple(){return Err(no());}
        let base=&held.site_ex;
        contents(base,PRODUCT)?;
        let authority=PublicAuthority::select(base)?;
        if SitePayload::decode(&authority.bytes)?!=payload{return Err(no());}
        let installed=Public::select(base,c"rust.install",1..=MAX_WIRE)?;
        let plan=RustPlan::decode(installed.bytes()).map_err(bad)?;
        if &plan!=expected||payload.origin!=plan.origin||payload.plan_sha!=plan.sha()||
            payload.system.as_path()!=held.system.path(){return Err(no());}
        let vault=super::product_pin(base,c"vault",false)?;
        let x=Self{held,installed,plan,vault,authority:Some((authority,payload))};
        x.require()?;contents(&x.offline_vault_directory()?,&[])?;Ok(x)
    }
    pub(in crate::sqlite_install::pair) fn fresh_payload(&self)->Option<&SitePayload>{
        self.authority.as_ref().map(|(_,p)|p)
    }
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{
        self.held.require()?;
        let site=&self.held.site_ex;
        let (original,_)=self.authority.as_ref().ok_or_else(no)?;original.require(site)?;
        contents(site,PRODUCT)?;
        self.installed.require(site,c"rust.install")?;
        let current=mark(&self.vault)?;
        let m=self.vault.metadata()?;
        let pin=||super::product_pin(site,c"vault",false);
        if m.mode()!=0o040700||m.uid()!=0||m.gid()!=0||m.nlink()==0||current.1!=mark(site)?.1
            ||current!=mark(&pin()?)?{return Err(no());}
        let dir=child(&self.vault,c".")?;
        if mark(&dir)?!=current{return Err(no());}
        layout::exact_with(site,c"vault",&self.vault,pin)?;
        layout::bare_acl(&dir,c"system.posix_acl_access")?;
        layout::bare_acl(&dir,c"system.posix_acl_default")?;
        contents(site,PRODUCT)?;self.held.require()
    }
    pub(in crate::sqlite_install::pair) fn plan(&self)->&RustPlan{&self.plan}
    pub(in crate::sqlite_install::pair) fn original_system(&self)->io::Result<(&Pinned,&File)>{
        self.require()?;Ok((&self.held.system,&self.held.system_lock))
    }
    pub(in crate::sqlite_install::pair) fn subject(&self)->([u8;16],u32,u32){self.held.subject}
    pub(in crate::sqlite_install::pair) fn offline_vault_directory(&self)->io::Result<File>{
        self.require()?;
        let actual=child(&self.vault,c".")?;
        if mark(&actual)?!=mark(&self.vault)?{return Err(no());}
        self.require()?;Ok(actual)
    }
}


/// Private borrow of the real owning original. Reset never constructs an
/// installed authority from a namestring, records, or an arbitrary descriptor.
#[derive(Clone,Copy)]
pub(in crate::sqlite_install::pair) enum CredentialSite<'s>{Fresh(&'s RustSite),Deployed(&'s DeployedSite)}
impl<'s> CredentialSite<'s>{
    pub(in crate::sqlite_install::pair) fn same(self,other:Self)->bool{
        match (self,other){
            (Self::Fresh(a),Self::Fresh(b))=>std::ptr::eq(a,b),
            (Self::Deployed(a),Self::Deployed(b))=>std::ptr::eq(a,b),
            _=>false,
        }
    }
    pub(in crate::sqlite_install::pair) fn require(self)->io::Result<()>{
        match self{Self::Fresh(f)=>f.require(),Self::Deployed(d)=>d.require()}
    }
    pub(in crate::sqlite_install::pair) fn plan(self)->&'s RustPlan{
        match self{Self::Fresh(f)=>f.plan(),Self::Deployed(d)=>d.plan()}
    }
    pub(in crate::sqlite_install::pair) fn original_system(self)->io::Result<(&'s Pinned,&'s File)>{
        match self{Self::Fresh(f)=>f.original_system(),Self::Deployed(d)=>{d.require()?;Ok(d.original())}}
    }
    pub(in crate::sqlite_install::pair) fn subject(self)->([u8;16],u32,u32){
        match self{Self::Fresh(f)=>f.subject(),Self::Deployed(d)=>d.payload().subject.tuple()}
    }
    pub(super) fn offline_vault_directory(self)->io::Result<File>{
        match self{Self::Fresh(f)=>f.offline_vault_directory(),Self::Deployed(d)=>d.offline_vault_directory()}
    }
}

/// Runtime-read public installation authority. It names the actual stable
/// allocator's site, not a per-user Helper setting or an operating grant.
pub(super) struct PublicAuthority{pin:File,stamp:super::live::Stamp,bytes:Vec<u8>}
fn authority_pin(dir:&File)->io::Result<File>{
    Ok(File::from(openat2(dir,AUTHORITY,OFlags::PATH|OFlags::CLOEXEC|OFlags::NOFOLLOW,Mode::empty(),
        ResolveFlags::BENEATH|ResolveFlags::NO_XDEV|ResolveFlags::NO_MAGICLINKS|ResolveFlags::NO_SYMLINKS)?))
}
impl PublicAuthority{
    pub(super) fn select(parent:&File)->io::Result<Self>{
        let pin=authority_pin(parent)?;let stamp=super::live::Stamp::on(&pin)?;
        let m=pin.metadata()?;
        if !m.is_file()||m.uid()!=0||m.gid()!=0||m.mode()!=0o100600||m.nlink()!=1||
            !(1..=MAX_SITE as u64).contains(&m.len())||stamp.mount!=mark(parent)?.1{return Err(no());}
        let bytes=Self::body(&pin,stamp)?;let x=Self{pin,stamp,bytes};x.require(parent)?;Ok(x)
    }
    fn body(pin:&File,wanted:super::live::Stamp)->io::Result<Vec<u8>>{
        let r=private_inode_reader(pin,0,wanted.mount)?;
        layout::bare_acl(&r,c"system.posix_acl_access")?;
        let mut b=Vec::new();(&r).take(MAX_SITE as u64+1).read_to_end(&mut b)?;
        if b.len() as u64!=wanted.len||super::live::Stamp::on(&r)?!=wanted||super::live::Stamp::on(pin)?!=wanted{
            return Err(no());
        }Ok(b)
    }
    pub(super) fn require(&self,parent:&File)->io::Result<()>{
        if mark(parent)?.1!=self.stamp.mount||super::live::Stamp::on(&authority_pin(parent)?)?!=self.stamp||
            Self::body(&self.pin,self.stamp)?!=self.bytes||super::live::Stamp::on(&authority_pin(parent)?)?!=self.stamp{
            return Err(no());
        }Ok(())
    }
    pub(super) fn bytes(&self)->&[u8]{&self.bytes}
}

/// Separate root verification in each existing worker, before final drop.
/// No site or vault FD is retained by the low SQL worker, and no secret or
/// deploy selector enters this check. Parent retains the actual EXs.
pub(in crate::sqlite_install::pair) fn verify_fresh_worker(system:&Path,plan:&RustPlan,scope:&CredentialScope)->io::Result<()>{
    require_v18()?;
    let site=Site::product(approval::host_admin()?,&plan.origin.instance,false)?;
    layout::verify_registered_instance(&site,scope.subject.tuple().2)?;
    let dir=child(site.base(),c".")?;
    if mark(&dir)?!=mark(site.base())?{return Err(no());}
    contents(&dir,PRODUCT)?;
    let record=PublicAuthority::select(&dir)?;
    let payload=SitePayload::decode(record.bytes())?;
    let installed=Public::select(&dir,c"rust.install",1..=MAX_WIRE)?;
    if payload.subject!=scope.subject||payload.origin!=plan.origin||payload.plan_sha!=plan.sha()||
        payload.system.as_path()!=system||payload.sha()?!=scope.site||installed.bytes()!=plan.wire(){return Err(no());}
    installed.require(&dir,c"rust.install")?;record.require(&dir)?;site.check()
}

#[derive(Clone,Copy,PartialEq,Eq)]
pub(in crate::sqlite_install::pair) struct VaultStamp{
    pub dev:u64,pub ino:u64,pub ctime_sec:i64,pub ctime_ns:u32,pub mtime_sec:i64,pub mtime_ns:u32,
}
pub(in crate::sqlite_install::pair) fn checked_vault_stamp(file:&File,mount:u64)->io::Result<VaultStamp>{
    let m=file.metadata()?;let (e,mid)=mark(file)?;
    if !m.is_file()||m.mode()!=0o100600||m.uid()!=0||m.gid()!=0||m.nlink()!=1||m.len()!=240||mid!=mount{
        return Err(no());
    }
    let ns=|v|u32::try_from(v).ok().filter(|n|*n<1_000_000_000).ok_or_else(no);
    Ok(VaultStamp{dev:e.dev,ino:e.ino,ctime_sec:m.ctime(),ctime_ns:ns(m.ctime_nsec())?,
        mtime_sec:m.mtime(),mtime_ns:ns(m.mtime_nsec())?})
}
