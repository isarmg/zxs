//! Cold selection of the *expected* installation domain for the original
//! SETUP_PAIR receipt. No service start, SAM call, secret-store open, password,
//! SQL write or approval is derived from this observation.
use super::{Site,pin_dir};
use super::super::{child,contents,live::{Live,Stamp,PROFILE},mark};
use super::super::super::LocalDomain;
use crate::{pinned::{Kind,Pinned},sqlite_install::{approval,bad,originals::{LockedSystem,SelectedSystem},
    pair::{History,PairRequest},request::Id}};
use rustix::fs::{FlockOperation,Mode,OFlags,flock,open,openat};
use sha2::{Digest,Sha256};
use std::{ffi::CStr,fs::File,io,os::unix::fs::{FileExt,MetadataExt},path::Path};

const SLOTS:&[&[u8]]=&[b"private",b"state",b"smb.conf",b"domain.bound"];
fn leaf(p:&File,n:&CStr)->io::Result<File>{
    Ok(File::from(openat(p,n,OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?))
}
fn current_root()->io::Result<File>{
    Ok(File::from(open(c"/",OFlags::PATH|OFlags::DIRECTORY|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?))
}
fn confirm_name(p:&File,n:&CStr,f:&File,expected:Stamp)->io::Result<()>{
    if Stamp::on(f)?!=expected||Stamp::on(&leaf(p,n)?)?!=expected{
        return Err(bad("an original public installation record or its name changed"));
    }Ok(())
}
fn read_bound(f:&File)->io::Result<[u8;128]>{
    let mut bytes=[0;128];f.read_exact_at(&mut bytes,0)?;
    loop{match f.read_at(&mut [0],128){
        Ok(0)=>return Ok(bytes),Ok(_)=>return Err(bad("unexpected data after the fixed origin record")),
        Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,Err(e)=>return Err(e),
    }}
}
struct OriginalFile{pin:File,stamp:Stamp}
impl OriginalFile{
    fn select(dir:&File,n:&CStr,len:u64)->io::Result<Self>{
        let pin=leaf(dir,n)?;let stamp=Stamp::on(&pin)?;
        if !stamp.at(&Stamp::on(dir)?,len,0o444,0){return Err(bad("not the root-owned, read-only, single-link original public record"));}
        confirm_name(dir,n,&pin,stamp)?;Ok(Self{pin,stamp})
    }
}
struct Names{
    site:Site,private:File,state:File,live:OriginalFile,bound:OriginalFile,
    actor:Id,uid:u32,gid:u32,pool:Id,init:Id,instance:Id,provision:Id,
}
impl Names{
    fn require(&self)->io::Result<()>{
        self.site.require_from(current_root()?)?;let p=self.site.base();
        for (f,n) in [(&self.private,c"private"),(&self.state,c"state")]{
            let m=f.metadata()?;
            if !m.is_dir()||m.uid()!=self.uid||m.mode()&0o7777!=0o700||m.nlink()==0
                ||mark(f)?.1!=mark(p)?.1||mark(&pin_dir(p,n)?)?!=mark(f)?{
                return Err(bad("changed or unprotected dedicated private/state directory"));
            }
        }
        confirm_name(p,c"smb.conf",&self.live.pin,self.live.stamp)?;
        confirm_name(p,c"domain.bound",&self.bound.pin,self.bound.stamp)
    }
}
/// Administrative site selection ends at O_PATH metadata. Before SQLite or
/// any installation file is read the caller permanently drops to storage.
/// A copied BOUND or a held lock never authorizes a credential write.
pub(in crate::sqlite_install::pair) struct SelectedSetup{system:SelectedSystem,names:Names}
impl SelectedSetup{
    pub(in crate::sqlite_install::pair) fn select(system:&Path,pool:Id,init:Id,instance:Id,provision:Id)->io::Result<Self>{
        if [pool,init,instance,provision].contains(&[0;16]){return Err(bad("missing independently selected original instance or pool identifiers"));}
        approval::no_inherited_descriptors()?;
        let host=approval::host_admin()?;let(actor,uid,gid)=approval::profile()?;
        approval::same_terminal(&io::stdin(),&io::stderr())?;
        let pin=Pinned::open(system,Kind::StorageRoot)?;let site=Site::select(host.try_clone()?,&instance)?;
        let base=site.base();let id=mark(base)?.0;
        if pin.visible_chain().contains(&id)||site.chain.iter().map(mark).collect::<io::Result<Vec<_>>>()?.iter().any(|a|a.0==pin.entity()){
            return Err(bad("the installed instance overlaps the original SYSTEM"));}
        let private=pin_dir(base,c"private")?;let state=pin_dir(base,c"state")?;
        let live=OriginalFile::select(base,c"smb.conf",PROFILE.len() as u64)?;
        let bound=OriginalFile::select(base,c"domain.bound",128)?;
        let ids=[pin.entity(),id,mark(&private)?.0,mark(&state)?.0,live.stamp.entity,bound.stamp.entity];
        if ids.iter().enumerate().any(|(i,v)|ids[..i].contains(v)){return Err(bad("aliased original SYSTEM or host installation roles"));}
        let names=Names{site,private,state,live,bound,actor,uid,gid,pool,init,instance,provision};
        names.require()?;let system=SelectedSystem::from_origin(pin,host,uid,gid)?;
        Ok(Self{system,names})
    }
    pub(in crate::sqlite_install::pair) fn storage_identity(&self)->(u32,u32){(self.names.uid,self.names.gid)}
    /// S first, then instance, both independent nonblocking EX. No new opens
    /// of private SAM files or of the SMB service process are involved.
    pub(in crate::sqlite_install::pair) fn after_drop(self)->io::Result<InstalledSetup>{
        let Self{system,names}=self;
        crate::maintenance::diagnostic_worker(names.uid,names.gid)?;
        names.require()?;let system=system.lock()?;
        let base=child(names.site.base(),c".")?;
        if mark(&base)?!=mark(names.site.base())?{return Err(bad("not the selected original installation"));}
        flock(&base,FlockOperation::NonBlockingLockExclusive)?;
        names.require()?;contents(&base,SLOTS)?;
        let sha=Sha256::digest(PROFILE).into();
        let live=Live::from_original(&base,&names.live.pin,&sha)?;
        let f=crate::pinned::sam_public_reader(&names.bound.pin,names.bound.stamp.mount,128)?;
        confirm_name(&base,c"domain.bound",&f,names.bound.stamp)?;let bytes=read_bound(&f)?;
        confirm_name(&base,c"domain.bound",&f,names.bound.stamp)?;
        let domain=LocalDomain::expected_binding(&bytes,&names.pool,&names.instance,&names.provision,&sha)?;
        let t=InstalledSetup{names,system,base,live,bound:f,bytes,domain};t.require()?;Ok(t)
    }
}
pub(in crate::sqlite_install::pair) struct InstalledSetup{
    names:Names,system:LockedSystem,base:File,live:Live,bound:File,bytes:[u8;128],domain:LocalDomain,
}
impl InstalledSetup{
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{
        self.system.require()?;self.names.require()?;
        if mark(&self.base)?!=mark(self.names.site.base())?{return Err(bad("lost original installed instance"));}
        contents(&self.base,SLOTS)?;self.live.require(&self.base)?;
        confirm_name(&self.base,c"domain.bound",&self.bound,self.names.bound.stamp)?;
        if read_bound(&self.bound)?!=self.bytes{return Err(bad("the original installed domain binding no longer matches"));}
        confirm_name(&self.base,c"domain.bound",&self.bound,self.names.bound.stamp)?;
        self.names.require()
    }
    pub(in crate::sqlite_install::pair) fn system(&self)->&LockedSystem{&self.system}
    pub(in crate::sqlite_install::pair) fn installation(&self)->(Id,Id){(self.names.instance,self.names.provision)}
    pub(in crate::sqlite_install::pair) fn request(&self,op:Id,approval:Id,web:&[u8],smb:&[u8])->io::Result<PairRequest>{
        self.require()?;let n=&self.names;
        if op==n.provision{return Err(bad("the original domain operation cannot be reused as setup"));}
        PairRequest::new(n.pool,n.init,op,n.actor,approval,web,smb,self.domain.expected_smb_sid())
    }
    /// This returns a historical result (or absence), never a writer. The SMB
    /// SID is derived from the installation record, never supplied by a user.
    pub(in crate::sqlite_install::pair) fn history(&self,op:Id,approval:Id,web:&[u8],smb:&[u8])->io::Result<(PairRequest,History)>{
        let r=self.request(op,approval,web,smb)?;
        let h=r.observe_offline(&self.system)?;self.require()?;Ok((r,h))
    }
}

#[cfg(test)]
mod tests{
    use super::*;use std::io::Write;
    // Disposable public files only. None is a root-issued binding.
    #[test]fn read_complete_public_bytes_but_reject_a_byteidentical_replacement_or_same_inode_change(){
        let d=tempfile::tempdir().unwrap();let dir=File::open(d.path()).unwrap();
        let p=d.path().join("domain.bound");let mut f=File::options().create_new(true).read(true).write(true).open(&p).unwrap();
        let value=[9;128];f.write_all(&value).unwrap();let known=Stamp::on(&f).unwrap();
        assert_eq!(read_bound(&f).unwrap(),value);confirm_name(&dir,c"domain.bound",&f,known).unwrap();
        std::fs::rename(&p,d.path().join("kept")).unwrap();std::fs::write(&p,value).unwrap();
        assert!(confirm_name(&dir,c"domain.bound",&f,known).is_err());
        let replacement=File::options().read(true).write(true).open(&p).unwrap();
        assert_eq!(read_bound(&replacement).unwrap(),value);
        let s=Stamp::on(&replacement).unwrap();confirm_name(&dir,c"domain.bound",&replacement,s).unwrap();
        replacement.write_all_at(b"x",128).unwrap();assert!(read_bound(&replacement).is_err());
        assert!(confirm_name(&dir,c"domain.bound",&replacement,s).is_err());
        replacement.set_len(127).unwrap();assert!(read_bound(&replacement).is_err());
    }
}
