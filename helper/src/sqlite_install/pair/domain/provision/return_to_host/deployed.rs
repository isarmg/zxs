//! A read-only, one-session owner for the ZXS deployment locator and the
//! authoritative existing site. No Helper state or credential is consulted.
use super::{PRODUCT,Site,approval,bad,child,contents,layout,lease,mark,product_pin,public_record::Public,
    rust_site::{PublicAuthority,checked_vault_stamp},shared_lease};
use crate::{pinned::{Kind,Pinned,private_inode_reader},smb_profile::rust_first::{MAX_WIRE,RustPlan},
    sqlite_install::pair::fresh_issuer::{SitePayload,compiled_v17,image}};
use rustix::fs::{FlockOperation,Mode,OFlags,ResolveFlags,flock,openat2};
use sha2::{Digest,Sha256};
use std::{ffi::CStr,fs::File,io,os::unix::fs::{FileExt,MetadataExt},path::Path};

const LOCATOR:&CStr=c"deployment.v1";
const VAULT:&CStr=c"vault";
const RECORD:&CStr=c"credential.v1";
const MAGIC:&[u8;16]=b"ZXS-DEPLOYED-V1\0";
fn unknown()->io::Error{bad("the original independently managed ZXS deployment was not confirmed")}
fn leaf(parent:&File,name:&CStr)->io::Result<File>{
    Ok(File::from(openat2(parent,name,OFlags::PATH|OFlags::CLOEXEC|OFlags::NOFOLLOW,Mode::empty(),
        ResolveFlags::BENEATH|ResolveFlags::NO_XDEV|ResolveFlags::NO_MAGICLINKS|ResolveFlags::NO_SYMLINKS)?))
}
fn protected_parent(d:&File)->io::Result<()>{
    let m=d.metadata()?;
    if !m.is_dir()||m.nlink()==0||m.uid()!=0||m.gid()!=0||!matches!(m.mode()&0o7777,0o700|0o755){return Err(unknown());}
    layout::bare_acl(d,c"system.posix_acl_access")?;
    layout::bare_acl(d,c"system.posix_acl_default")
}
fn locate(fd:&File)->io::Result<([u8;16],[u8;32])>{
    let mut v=[0;64];
    fd.read_exact_at(&mut v,0)?;
    if fd.read_at(&mut [0],64)?!=0{return Err(unknown());}
    decode_locator(&v)
}
fn decode_locator(v:&[u8])->io::Result<([u8;16],[u8;32])>{
    if v.len()!=64||&v[..16]!=MAGIC||v[16..32]==[0;16]||v[32..]==[0;32]{return Err(unknown());}
    Ok((v[16..32].try_into().expect("fixed site identity"),v[32..].try_into().expect("fixed site digest")))
}
fn locator(parent:&File)->io::Result<(File,([u8;16],[u8;32]))>{
    let pin=leaf(parent,LOCATOR)?;let m=pin.metadata()?;
    if m.mode()!=0o100600||m.uid()!=0||m.gid()!=0||m.nlink()!=1||m.len()!=64{return Err(unknown());}
    layout::exact_with(parent,LOCATOR,&pin,||leaf(parent,LOCATOR))?;
    let stamp=super::live::Stamp::on(&pin)?;
    let reader=private_inode_reader(&pin,0,mark(parent)?.1)?;
    layout::bare_acl(&reader,c"system.posix_acl_access")?;
    let value=locate(&reader)?;
    if super::live::Stamp::on(&reader)?!=stamp||super::live::Stamp::on(&leaf(parent,LOCATOR)?)?!=stamp{
        return Err(unknown());
    }
    Ok((pin,value))
}

#[must_use]
pub(in crate::sqlite_install::pair) struct DeployedSite{
    parent:File,selector:File,selection:([u8;16],[u8;32]),original:Site,site_ex:File,
    system:Pinned,system_ex:File,authority:PublicAuthority,plan_file:Public,
    payload:SitePayload,plan:RustPlan,verified_image:image::Image,
}
impl DeployedSite{
    pub(in crate::sqlite_install::pair) fn select()->io::Result<Self>{
        compiled_v17()?;
        approval::no_inherited_descriptors()?;
        approval::same_terminal(&io::stdin(),&io::stderr())?;
        let host=approval::host_admin()?;
        let mut base=host.try_clone()?;
        for (i,name) in PRODUCT.into_iter().enumerate(){base=product_pin(&base,name,i<2)?;}
        let parent=child(&base,c".")?;
        if mark(&base)?!=mark(&parent)?{return Err(unknown());}
        protected_parent(&parent)?;
        flock(&parent,FlockOperation::NonBlockingLockShared)?;
        let (selector,selection)=locator(&parent)?;
        let original=Site::product(host,&selection.0,false)?;
        if mark(&original.chain[PRODUCT.len()-1])?!=mark(&parent)?{return Err(unknown());}
        let site_ex=child(original.base(),c".")?;
        let authority=PublicAuthority::select(&site_ex)?;
        let payload=SitePayload::decode(authority.bytes())?;
        if payload.origin.instance!=selection.0||<[u8;32]>::from(Sha256::digest(authority.bytes()))!=selection.1||
            payload.image.as_path()!=Path::new(image::PATH){return Err(unknown());}
        layout::verify_registered_instance(&original,payload.subject.tuple().2)?;
        let plan_file=Public::select(&site_ex,c"rust.install",1..=MAX_WIRE)?;
        let plan=RustPlan::decode(plan_file.bytes()).map_err(bad)?;
        if plan.origin!=payload.origin||plan.sha()!=payload.plan_sha{return Err(unknown());}
        let verified_image=image::Image::select(&payload.elf_sha)?;
        let system=Pinned::already_mounted(&payload.system,Kind::StorageRoot)?;
        let system_ex=system.storage_directory()?;
        // Same kernel order as the only original cold supervisor. An online
        // participant holds S SH, in which case this check cannot qualify.
        flock(&system_ex,FlockOperation::NonBlockingLockExclusive)?;
        flock(&site_ex,FlockOperation::NonBlockingLockExclusive)?;
        let result=Self{parent,selector,selection,original,site_ex,system,system_ex,
            authority,plan_file,payload,plan,verified_image};
        result.require()?;Ok(result)
    }
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{
        compiled_v17()?;
        approval::same_terminal(&io::stdin(),&io::stderr())?;
        let (_,uid,gid)=self.payload.subject.tuple();
        protected_parent(&self.parent)?;
        layout::verify_registered_instance(&self.original,gid)?;
        shared_lease(&self.parent,mark(&self.original.chain[PRODUCT.len()-1])?)?;
        let s=self.system.require_same_named_mount()?;
        if !self.system.passive_mount_observation()||self.system.metadata()?.uid()!=uid||
            self.system.metadata()?.mode()!=0o040700||self.payload.system.as_path()!=self.system.path(){
            return Err(unknown());
        }
        lease(&self.system_ex,(self.system.entity(),s.id))?;
        lease(&self.site_ex,mark(self.original.base())?)?;
        layout::bare_acl(&self.site_ex,c"system.posix_acl_default")?;
        // Both names are retained even if a future product pair uses one
        // physical backing; never let an origin be inside the other role.
        if self.system.visible_chain().contains(&mark(self.original.base())?.0)||
            mark(&self.original.host)?.0==self.system.entity(){return Err(unknown());}
        for path in &self.original.chain {if mark(path)?.0==self.system.entity(){return Err(unknown());}}
        let (located,value)=locator(&self.parent)?;
        if value!=self.selection||super::live::Stamp::on(&located)?!=super::live::Stamp::on(&self.selector)?{
            return Err(unknown());
        }
        contents(&self.site_ex,&[b"rust.install",b"site.v1",b"vault"])?;
        self.authority.require(&self.site_ex)?;self.plan_file.require(&self.site_ex,c"rust.install")?;
        self.verified_image.require()
    }
    pub(in crate::sqlite_install::pair) fn original(&self)->(&Pinned,&File){(&self.system,&self.system_ex)}
    pub(in crate::sqlite_install::pair) fn payload(&self)->&SitePayload{&self.payload}
    pub(in crate::sqlite_install::pair) fn plan(&self)->&RustPlan{&self.plan}
    pub(in crate::sqlite_install::pair) fn site_sha(&self)->[u8;32]{self.selection.1}
    pub(in crate::sqlite_install::pair) fn require_vault_prefix(&self,wanted:&[u8;224])->io::Result<()>{
        self.require()?;
        let vault=product_pin(&self.site_ex,VAULT,false)?;
        let m=vault.metadata()?;
        if m.mode()!=0o040700||m.uid()!=0||m.gid()!=0{return Err(unknown());}
        let dir=child(&vault,c".")?;
        if mark(&dir)?!=mark(&vault)?{return Err(unknown());}
        layout::bare_acl(&dir,c"system.posix_acl_access")?;layout::bare_acl(&dir,c"system.posix_acl_default")?;
        contents(&dir,&[b"credential.v1"])?;
        let original=leaf(&dir,RECORD)?;let mount=mark(&dir)?.1;
        let stamp=checked_vault_stamp(&original,mount)?;
        let read=private_inode_reader(&original,0,mount)?;
        layout::bare_acl(&read,c"system.posix_acl_access")?;
        let mut prefix=[0;224];read.read_exact_at(&mut prefix,0)?; // never the 16-byte credential
        if &prefix!=wanted||checked_vault_stamp(&read,mount)?!=stamp||
            checked_vault_stamp(&leaf(&dir,RECORD)?,mount)?!=stamp||mark(&product_pin(&self.site_ex,VAULT,false)?)?!=mark(&dir)?{
            return Err(unknown());
        }
        self.require()
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn reattach_only_the_exact_complete_nonzero_zxs_selector(){
        let mut bytes=[0;64];bytes[..16].copy_from_slice(MAGIC);bytes[16..32].fill(1);bytes[32..].fill(2);
        assert_eq!(decode_locator(&bytes).unwrap(),([1;16],[2;32]));
        assert!(decode_locator(&bytes[..63]).is_err());
        assert!(decode_locator(&[bytes.as_slice(),b"x"].concat()).is_err());
        bytes[32..].fill(0);assert!(decode_locator(&bytes).is_err());
    }
}
