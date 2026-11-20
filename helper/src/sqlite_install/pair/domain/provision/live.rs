//! Exact host-side first-install source. Host file ownership is not proof that
//! the separate, live smbd actually mounted it readonly or used this parser.
use crate::{pinned::{Entity,Mount},smb_profile::{self,first::{Origin,Plan}},sqlite_install::bad};
use rustix::fs::{Mode,OFlags,openat};
use sha2::{Digest,Sha256};
use std::{fs::File,io,os::unix::fs::{FileExt,MetadataExt}};

pub(super) const PROFILE:&[u8]=include_bytes!("../../../../../resources/samba-domain/initial-live.conf");
#[derive(Copy,Clone,PartialEq,Eq)]
pub(super) struct Stamp{pub(super) entity:Entity,pub(super) mount:u64,pub(super) mode:u32,pub(super) uid:u32,
    pub(super) links:u64,pub(super) len:u64,pub(super) mtime:(i64,i64),pub(super) ctime:(i64,i64)}
impl Stamp{
    pub(super) fn on(f:&File)->io::Result<Self>{let m=f.metadata()?;Ok(Self{entity:Entity::of(&m)?,
        mount:Mount::on(f)?.ok_or_else(||bad("missing installed live configuration mount"))?.id,
        mode:m.mode(),uid:m.uid(),links:m.nlink(),len:m.len(),
        mtime:(m.mtime(),m.mtime_nsec()),ctime:(m.ctime(),m.ctime_nsec())})}
    pub(super) fn at(&self,parent:&Stamp,length:u64,mode:u32,uid:u32)->bool{
        self.mode==(0o100000|mode)&&self.uid==uid&&self.links==1&&self.len==length
            &&self.mount==parent.mount&&self.entity.dev==parent.entity.dev&&self.entity!=parent.entity
    }
}
fn source_at(parent:&File)->io::Result<File>{
    Ok(File::from(openat(parent,c"smb.conf",OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?))
}
pub(super) fn exact(bytes:&File)->io::Result<()>{
    let mut seen=vec![0;PROFILE.len()];bytes.read_exact_at(&mut seen,0)?;
    if seen!=PROFILE{return Err(bad("installed live configuration is not the fixed first-install profile"));}
    let mut extra=[0];
    loop{match bytes.read_at(&mut extra,PROFILE.len() as u64){
        Ok(0)=>return Ok(()),Ok(_)=>return Err(bad("additional bytes after live configuration")),
        Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,Err(e)=>return Err(e),
    }}
}
fn valid(parent:&File,stamp:Stamp,pin:&File)->io::Result<bool>{
    Ok((1..=smb_profile::LIVE_MAX as u64).contains(&stamp.len)
        &&stamp.at(&Stamp::on(parent)?,stamp.len,0o444,0)&&pin.metadata()?.gid()==0)
}
fn read(f:&File,stamp:Stamp)->io::Result<Vec<u8>>{
    super::return_to_host::bare_acl(f,c"system.posix_acl_access")?;
    if Stamp::on(f)?!=stamp||f.metadata()?.gid()!=0{return Err(bad("changed exact original public config"));}
    let bytes=bounded_bytes(f,stamp.len)?;
    if Stamp::on(f)?!=stamp||f.metadata()?.gid()!=0{
        return Err(bad("changed or incomplete original live config"));
    }
    Ok(bytes)
}
fn bounded_bytes(f:&File,len:u64)->io::Result<Vec<u8>>{
    if !(1..=smb_profile::LIVE_MAX as u64).contains(&len){return Err(bad("unqualified public configuration size"));}
    let mut bytes=vec![0;len as usize];
    f.read_exact_at(&mut bytes,0)?; // same actual FD, never a mutable stream offset
    loop{match f.read_at(&mut [0],len){
        Ok(0)=>return Ok(bytes),Ok(_)=>return Err(bad("unexpected live config tail")),
        Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,Err(e)=>return Err(e),
    }}
}
fn original(parent:&File)->io::Result<(File,Stamp,Vec<u8>)>{
    let os=source_at(parent)?;let stamp=Stamp::on(&os)?;
    if !valid(parent,stamp,&os)?{return Err(bad("not the original root-owned, bounded 0444 configuration"));}
    let fd=crate::pinned::sam_public_reader(&os,stamp.mount,stamp.len)?;
    let bytes=read(&fd,stamp)?;
    if Stamp::on(&os)?!=stamp||Stamp::on(&source_at(parent)?)?!=stamp{
        return Err(bad("selected live configuration was replaced before opening"));
    }
    Ok((fd,stamp,bytes))
}
pub(super) struct Live{fd:File,stamp:Stamp,bytes:Vec<u8>,plan:Plan}
impl Live{
    pub(super) fn select(parent:&File,approved_sha:&[u8;32],expected:Origin)->io::Result<Self>{
        let (fd,stamp,bytes)=original(parent)?;
        if approved_sha==&[0;32]||&<[u8;32]>::from(Sha256::digest(&bytes))!=approved_sha{
            return Err(bad("not the exact live config from the independently selected layout receipt"));
        }
        let plan=smb_profile::exact_plan(&bytes).map_err(bad)?;
        if plan.origin!=expected{return Err(bad("different pool, INIT, instance, provisioning or first setup operation"));}
        let live=Self{fd,stamp,bytes,plan};live.require(parent)?;Ok(live)
    }
    pub(super) fn origin(&self)->(Entity,u64){(self.stamp.entity,self.stamp.mount)}
    pub(super) fn plan(&self)->&Plan{&self.plan}
    pub(super) fn require(&self,parent:&File)->io::Result<()>{
        let check=||->io::Result<()>{
            let name=source_at(parent)?;
            if !valid(parent,self.stamp,&self.fd)?||Stamp::on(&self.fd)?!=self.stamp||Stamp::on(&name)?!=self.stamp{
                return Err(bad("dedicated original live configuration changed"));}
            Ok(())
        };
        check()?;
        if read(&self.fd,self.stamp)?!=self.bytes{return Err(bad("dedicated original live configuration bytes changed"));}
        check()
    }
}
#[cfg(test)]
mod tests{
    use super::*;use std::io::Write;
    #[test]
    fn complete_default_source_and_valid_boot_profile_are_not_interchangeable(){
        let mut f=tempfile::tempfile().unwrap();f.write_all(PROFILE).unwrap();exact(&f).unwrap();
        f.write_all(b"\n").unwrap();assert!(exact(&f).is_err());
        f.set_len(PROFILE.len()as u64).unwrap();f.write_all_at(b"[homes] ",0).unwrap();assert!(exact(&f).is_err());
        let mut other=tempfile::tempfile().unwrap();other.write_all(super::super::BOOT).unwrap();assert!(exact(&other).is_err());
    }
    #[test]
    fn repeated_checks_of_the_same_held_filedescription_are_positional_and_require_eof(){
        let mut f=tempfile::tempfile().unwrap();f.write_all(b"same").unwrap();
        for _ in 0..2{assert_eq!(bounded_bytes(&f,4).unwrap(),b"same");}
        assert!(bounded_bytes(&f,3).is_err());assert!(bounded_bytes(&f,5).is_err());
        assert!(bounded_bytes(&f,(smb_profile::LIVE_MAX+1) as u64).is_err());
    }
    #[test]
    fn installed_record_has_one_link_root_owner_same_mount_and_exact_mode(){
        let parent=Stamp{entity:Entity{dev:42,ino:12},mount:8,mode:0o40755,uid:0,links:2,len:1024,mtime:(1,0),ctime:(1,0)};
        let valid=Stamp{entity:Entity{dev:42,ino:14},mode:0o100444,links:1,len:PROFILE.len() as u64,..parent};
        assert!(valid.at(&parent,PROFILE.len() as u64,0o444,0));
        for changed in [Stamp{uid:1000,..valid},Stamp{mode:0o100644,..valid},Stamp{links:2,..valid},Stamp{mount:9,..valid},Stamp{entity:parent.entity,..valid}]{
            assert!(!changed.at(&parent,PROFILE.len() as u64,0o444,0));
        }
    }
}
