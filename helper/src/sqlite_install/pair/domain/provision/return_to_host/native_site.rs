//! Original host installation for the one foreground native writer. Root
//! never opens the business SQLite file. It pins both directory leases and
//! gives only the already held original SYSTEM OFD to the low SQL process.
use super::{Site,accountdb::AccountParent,approval,bad,child,contents,layout,lease,live::{Stamp,PROFILE},mark,pin_dir};
use crate::{pinned::{Kind,Pinned},sqlite_install::{pair::domain::LocalDomain,request::Id}};
use rustix::fs::{FlockOperation,Mode,OFlags,StatVfsMountFlags,openat,flock,fstatvfs};
use sha2::{Digest,Sha256};
use std::{ffi::CStr,fs::File,io::{self,Read},os::unix::fs::MetadataExt,path::Path};

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
    fn select(parent:&File,name:&CStr,len:usize)->io::Result<Self>{
        let pin=path(parent,name)?;let stamp=Stamp::on(&pin)?;
        if !stamp.at(&Stamp::on(parent)?,len as u64,0o444,0){return Err(bad("not the exact protected public installation object"));}
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
    expected:(Id,Id,Id), domain:[u8;24],
}
impl NativeSite {
    pub(in crate::sqlite_install::pair) fn acquire(system:&Path,pool:Id,instance:Id,provision:Id)->io::Result<Self>{
        approval::no_inherited_descriptors()?;
        approval::same_terminal(&io::stdin(),&io::stderr())?;
        let root=approval::host_admin()?;let subject=approval::profile()?;
        let system=Pinned::open(system,Kind::StorageRoot)?;
        let s=system.metadata()?;
        if !s.is_dir()||s.uid()!=subject.1||s.mode()!=0o040700||s.nlink()==0{
            return Err(bad("not the true storage owner's original SYSTEM"));
        }
        let original=Site::select(root,&instance)?;
        layout::verify_registered_instance(&original,subject.2)?;
        let system_ex=system.storage_directory()?;
        flock(&system_ex,FlockOperation::NonBlockingLockExclusive)?;
        let site_ex=child(original.base(),c".")?;
        if mark(&site_ex)?!=mark(original.base())?{return Err(bad("changed original host instance"));}
        flock(&site_ex,FlockOperation::NonBlockingLockExclusive)?;
        contents(&site_ex,SLOTS)?;
        let config=Public::select(&site_ex,c"smb.conf",PROFILE.len())?;
        if config.value!=PROFILE{return Err(bad("not the fixed native-account Samba configuration"));}
        let bound=Public::select(&site_ex,c"domain.bound",128)?;
        let domain=LocalDomain::expected_binding(&bound.value,&pool,&instance,&provision,&Sha256::digest(PROFILE).into())?;
        let private=pin_dir(&site_ex,c"private")?;let state=pin_dir(&site_ex,c"state")?;
        let mut sid=[0;24];sid.copy_from_slice(&domain.0);
        let selected=Self{system,system_ex,original,site_ex,subject,config,bound,private,state,
            expected:(pool,instance,provision),domain:sid};
        selected.require()?;selected.accounts()?.leaf_pin()?;Ok(selected)
    }
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{
        layout::verify_registered_instance(&self.original,self.subject.2)?;
        if approval::profile()?!=self.subject||!self.system.metadata()?.is_dir()
            || self.system.metadata()?.uid()!=self.subject.1||self.system.metadata()?.mode()!=0o040700{
            return Err(bad("SYSTEM authorization, scope or writable mount changed"));
        }
        let s=mark(&self.system.storage_directory()?)?;lease(&self.system_ex,s)?;
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
        let (pool,instance,provision)=self.expected;
        LocalDomain::expected_binding(&self.bound.value,&pool,&instance,&provision,&self.live_digest())?;
        Ok(())
    }
    pub(in crate::sqlite_install::pair) fn subject(&self)->(Id,u32,u32){self.subject}
    pub(in crate::sqlite_install::pair) fn accounts(&self)->io::Result<AccountParent<'_>>{
        AccountParent::select(&self.system,&self.system_ex,&self.original,&self.site_ex)
    }
    pub(in crate::sqlite_install::pair) fn live_digest(&self)->[u8;32]{Sha256::digest(PROFILE).into()}
    pub(in crate::sqlite_install::pair) fn bound_bytes(&self)->[u8;128]{self.bound.value.as_slice().try_into().expect("fixed BOUND")}
    pub(in crate::sqlite_install::pair) fn system_lease(&self)->&File{&self.system_ex}
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
