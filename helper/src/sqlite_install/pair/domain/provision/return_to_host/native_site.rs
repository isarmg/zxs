//! Original host installation for the one foreground native writer. Root
//! never opens the business SQLite file. It pins both directory leases and
//! gives only the already held original SYSTEM OFD to the low SQL process.
use super::{Site,accountdb::AccountParent,approval,bad,child,contents,layout,lease,shared_lease,live::Stamp,mark,pin_dir};
use crate::{pinned::{Kind,Pinned},smb_profile::{self,first::{Origin,Plan}},sqlite_install::{pair::domain::LocalDomain,request::Id}};
use rustix::fs::{FlockOperation,Mode,OFlags,StatVfsMountFlags,openat,flock,fstatvfs};
use sha2::{Digest,Sha256};
use std::{ffi::CStr,fs::File,io::{self,Read},ops::RangeInclusive,os::unix::fs::MetadataExt,path::Path};

const SLOTS:&[&[u8]]=&[b"private",b"state",b"smb.conf",b"domain.bound",b"accountdb"];

fn path(parent:&File,name:&CStr)->io::Result<File>{
    Ok(File::from(openat(parent,name,OFlags::PATH|OFlags::CLOEXEC|OFlags::NOFOLLOW,Mode::empty())?))
}
struct Public {pin:File,stamp:Stamp,value:Vec<u8>}
impl Public {
    fn read_value(pin:&File,stamp:Stamp,len:usize)->io::Result<Vec<u8>>{
        if pin.metadata()?.gid()!=0{return Err(bad("not the original root-owned public installation record"));}
        let f=crate::pinned::sam_public_reader(pin,stamp.mount,len as u64)?;
        layout::bare_acl(&f,c"system.posix_acl_access")?;
        let mut b=Vec::with_capacity(len);
        (&f).take(len as u64+1).read_to_end(&mut b)?;
        if b.len()!=len||Stamp::on(&f)?!=stamp||Stamp::on(pin)?!=stamp{return Err(bad("changed public installation record"));}
        Ok(b)
    }
    fn select(parent:&File,name:&CStr,lengths:RangeInclusive<usize>)->io::Result<Self>{
        let pin=path(parent,name)?;let stamp=Stamp::on(&pin)?;
        let len=usize::try_from(stamp.len).map_err(|_|bad("invalid protected public installation record length"))?;
        if !lengths.contains(&len)||!stamp.at(&Stamp::on(parent)?,stamp.len,0o444,0){
            return Err(bad("not the exact protected public installation object"));
        }
        let value=Self::read_value(&pin,stamp,len)?;
        let v=Self{pin,stamp,value};v.require(parent,name)?;Ok(v)
    }
    fn require(&self,parent:&File,name:&CStr)->io::Result<()>{
        let same=||Stamp::on(&path(parent,name)?);
        if same()?!=self.stamp||Stamp::on(&self.pin)?!=self.stamp
            || Self::read_value(&self.pin,self.stamp,self.value.len())?!=self.value||same()?!=self.stamp{
            return Err(bad("original installed configuration or BOUND changed"));
        }Ok(())
    }
}
pub(in crate::sqlite_install::pair) struct NativeSite{
    system:Pinned,system_ex:File,original:Site,site_ex:File,subject:(Id,u32,u32),
    config:Public,bound:Public,private:File,state:File,
    plan:Plan, config_sha:[u8;32], domain:[u8;24], online:bool,
}
fn approved_live(bound:&[u8],config:&[u8],expected:Origin,receipt_sha:&[u8;32])->io::Result<(Plan,[u8;32],LocalDomain)>{
    if receipt_sha==&[0;32]||&<[u8;32]>::from(Sha256::digest(bound))!=receipt_sha{
        return Err(bad("BOUND is not the independent exact original provisioning receipt"));
    }
    let plan=smb_profile::exact_plan(config).map_err(bad)?;
    if plan.origin!=expected{return Err(bad("the approved live plan has a different pool, INIT, site, domain or SETUP origin"));}
    let live=Sha256::digest(config).into();
    let domain=LocalDomain::expected_binding(bound,&expected.pool,&expected.instance,&expected.domain,&live)?;
    Ok((plan,live,domain))
}
impl NativeSite {
    pub(in crate::sqlite_install::pair) fn acquire(system:&Path,expected:Origin,receipt_sha:&[u8;32])->io::Result<Self>{
        Self::select(system,expected,receipt_sha,false)
    }
    pub(in crate::sqlite_install::pair) fn acquire_online(system:&Path,expected:Origin,receipt_sha:&[u8;32])->io::Result<Self>{
        Self::select(system,expected,receipt_sha,true)
    }
    fn select(system:&Path,expected:Origin,receipt_sha:&[u8;32],online:bool)->io::Result<Self>{
        approval::no_inherited_descriptors()?;
        approval::same_terminal(&io::stdin(),&io::stderr())?;
        let root=approval::host_admin()?;let subject=approval::profile()?;
        let system=Pinned::open(system,Kind::StorageRoot)?;
        let s=system.metadata()?;
        if !s.is_dir()||s.uid()!=subject.1||s.mode()!=0o040700||s.nlink()==0{
            return Err(bad("not the true storage owner's original SYSTEM"));
        }
        let original=Site::select(root,&expected.instance)?;
        layout::verify_registered_instance(&original,subject.2)?;
        let system_ex=system.storage_directory()?;
        let lock=if online{FlockOperation::NonBlockingLockShared}else{FlockOperation::NonBlockingLockExclusive};
        flock(&system_ex,lock)?;
        let site_ex=child(original.base(),c".")?;
        if mark(&site_ex)?!=mark(original.base())?{return Err(bad("changed original host instance"));}
        flock(&site_ex,FlockOperation::NonBlockingLockExclusive)?;
        contents(&site_ex,SLOTS)?;
        let bound=Public::select(&site_ex,c"domain.bound",128..=128)?;
        if receipt_sha==&[0;32]||&<[u8;32]>::from(Sha256::digest(&bound.value))!=receipt_sha{
            return Err(bad("BOUND is not the independent exact original provisioning receipt"));
        }
        let config=Public::select(&site_ex,c"smb.conf",1..=smb_profile::LIVE_MAX)?;
        let (plan,config_sha,domain)=approved_live(&bound.value,&config.value,expected,receipt_sha)?;
        let private=pin_dir(&site_ex,c"private")?;let state=pin_dir(&site_ex,c"state")?;
        let mut sid=[0;24];sid.copy_from_slice(&domain.0);
        let selected=Self{system,system_ex,original,site_ex,subject,config,bound,private,state,
            plan,config_sha,domain:sid,online};
        selected.require()?;
        if !online{selected.accounts()?.leaf_pin()?;}
        Ok(selected)
    }
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{
        layout::verify_registered_instance(&self.original,self.subject.2)?;
        self.system.require_same_named_mount()?;
        if approval::profile()?!=self.subject||!self.system.metadata()?.is_dir()
            || self.system.metadata()?.uid()!=self.subject.1||self.system.metadata()?.mode()!=0o040700{
            return Err(bad("SYSTEM authorization, scope or writable mount changed"));
        }
        let s=mark(&self.system.storage_directory()?)?;
        if self.online{shared_lease(&self.system_ex,s)?;}else{lease(&self.system_ex,s)?;}
        let site=mark(self.original.base())?;lease(&self.site_ex,site)?;
        if self.system.visible_chain().contains(&site.0)
            || self.original.chain.iter().map(mark).collect::<io::Result<Vec<_>>>()?.iter().any(|(e,_)|*e==s.0){
            return Err(bad("the host instance and SYSTEM overlap"));
        }
        contents(&self.site_ex,SLOTS)?;
        for (file,name) in [(&self.private,c"private"),(&self.state,c"state")]{
            let m=file.metadata()?;let loc=mark(file)?;
            if m.uid()!=self.subject.1||m.gid()!=self.subject.2||m.mode()!=0o040700||m.nlink()==0||loc.0.dev!=site.0.dev
                ||loc.1!=site.1||loc!=mark(&pin_dir(&self.site_ex,name)?)?{
                return Err(bad("wrong original standalone private or state role"));
            }
            let actual=child(file,c".")?;
            if mark(&actual)?!=loc{return Err(bad("changed dedicated machine directory"));}
            layout::bare_acl(&actual,c"system.posix_acl_access")?;
            layout::bare_acl(&actual,c"system.posix_acl_default")?;
            layout::exact_component(&self.site_ex,name,file)?;
        }
        self.config.require(&self.site_ex,c"smb.conf")?;
        self.bound.require(&self.site_ex,c"domain.bound")?;
        let o=self.plan.origin;
        LocalDomain::expected_binding(&self.bound.value,&o.pool,&o.instance,&o.domain,&self.config_sha)?;
        Ok(())
    }
    pub(in crate::sqlite_install::pair) fn subject(&self)->(Id,u32,u32){self.subject}
    pub(in crate::sqlite_install::pair) fn accounts(&self)->io::Result<AccountParent<'_>>{
        if self.online{return Err(bad("a running-site lease never authorizes native account maintenance"));}
        AccountParent::select(&self.system,&self.system_ex,&self.original,&self.site_ex)
    }
    pub(in crate::sqlite_install::pair) fn live_digest(&self)->[u8;32]{self.config_sha}
    pub(in crate::sqlite_install::pair) fn first_plan(&self)->&Plan{&self.plan}
    pub(in crate::sqlite_install::pair) fn bound_bytes(&self)->[u8;128]{self.bound.value.as_slice().try_into().expect("fixed BOUND")}
    pub(in crate::sqlite_install::pair) fn system_lease(&self)->&File{&self.system_ex}
    pub(in crate::sqlite_install::pair) fn online_sources(&self)->io::Result<(&Pinned,&File,&File,&File,&File)>{
        if !self.online{return Err(bad("not the actual shared SYSTEM and exclusive online site"));}
        self.require()?;
        Ok((&self.system,&self.system_ex,&self.site_ex,&self.private,&self.state))
    }
    pub(in crate::sqlite_install::pair) fn system_location(&self)->io::Result<[u64;3]>{
        let (e,m)=mark(&self.system_ex)?;Ok([e.dev,e.ino,m])
    }
    pub(in crate::sqlite_install::pair) fn require_write(&self)->io::Result<()>{
        self.require()?;
        let target=self.accounts()?.leaf_pin()?;
        for f in [&target,&self.system_ex]{
            if fstatvfs(f)?.f_flag.contains(StatVfsMountFlags::RDONLY){return Err(bad("an affected original mount is read-only"));}
        }
        Ok(())
    }
    pub(in crate::sqlite_install::pair) fn original_machine(&self)->io::Result<File>{
        self.require()?;
        let m=path(&self.private,c"secrets.tdb")?;
        self.require_machine(&m)?;Ok(m)
    }
    pub(in crate::sqlite_install::pair) fn require_machine(&self,machine:&File)->io::Result<()>{
        let m=machine.metadata()?;let place=mark(machine)?;
        if m.uid()!=self.subject.1||m.gid()!=self.subject.2||m.mode()!=0o100600||m.nlink()!=1
            ||!(1..=64<<20).contains(&m.len())||place.1!=mark(&self.site_ex)?.1
            ||place!=mark(&path(&self.private,c"secrets.tdb")?)?{
            return Err(bad("changed, unknown or unprotected original local-domain database"));
        }
        let mfd=crate::pinned::private_inode_reader(machine,self.subject.1,place.1)?;
        layout::bare_acl(&mfd,c"system.posix_acl_access")?;
        if place!=mark(&mfd)?||place!=mark(&path(&self.private,c"secrets.tdb")?)?{return Err(bad("changed original domain store"));}
        Ok(())
    }
    pub(in crate::sqlite_install::pair) fn public_pins(&self)->[&File;2]{[&self.config.pin,&self.bound.pin]}
    pub(in crate::sqlite_install::pair) fn host_root(&self)->&File{&self.original.host}
    pub(in crate::sqlite_install::pair) fn machine_scope(&self)->(&File,&File,&[u8;24]){
        (self.original.base(),&self.private,&self.domain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smb_profile::first::{Endpoint,Folder,Mode};

    fn config(o:Origin,label:&str)->String{
        let p=Plan::selected(o,vec![Endpoint::named("192.0.2.17/24").unwrap()],
            vec![Folder{volume:[0x21;16],name:label.to_owned(),mode:Mode::ReadOnly}]).unwrap();
        smb_profile::render(&p).unwrap()
    }
    fn bound(o:Origin,cfg:&str)->[u8;128]{
        let mut b=[0;128];
        b[..16].copy_from_slice(b"ZXS-SAM/BOUND-1\0");
        b[16..32].copy_from_slice(&o.pool);
        b[32..48].copy_from_slice(&o.instance);
        b[48..64].copy_from_slice(&o.domain);
        b[64..96].copy_from_slice(&Sha256::digest(cfg.as_bytes()));
        b[96..104].copy_from_slice(b"ZXSLOCAL");
        b[104..116].copy_from_slice(&[1,4,0,0,0,0,0,5,21,0,0,0]);
        b[116..].fill(1);b
    }
    #[test] fn independent_receipt_and_full_original_site_scope_are_required(){
        let o=Origin{pool:[1;16],init:[2;16],instance:[3;16],domain:[4;16],setup:[5;16]};
        let c=config(o,"Docs");let b=bound(o,&c);let receipt=Sha256::digest(b).into();
        assert_eq!(approved_live(&b,c.as_bytes(),o,&receipt).unwrap().0.origin,o);
        // A plausible replacement config and its jointly replaced BOUND are
        // not the original receipt. A reset's own Op is not the first Setup.
        let new=config(o,"Other");let replacement=bound(o,&new);
        assert!(approved_live(&replacement,new.as_bytes(),o,&receipt).is_err());
        assert!(approved_live(&b,new.as_bytes(),o,&receipt).is_err());
        assert!(approved_live(&b,c.as_bytes(),Origin{setup:[6;16],..o},&receipt).is_err());
        let legacy=super::super::super::live::PROFILE;
        assert!(approved_live(&b,legacy,o,&receipt).is_err());
    }
}
