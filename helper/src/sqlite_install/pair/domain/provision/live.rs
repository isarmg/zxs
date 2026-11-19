//! Exact host-side first-install source. Host file ownership is not proof that
//! the separate, live smbd actually mounted it readonly or used this parser.
use crate::{pinned::{Entity,Mount},sqlite_install::bad};
use rustix::fs::{Mode,OFlags,openat};
use sha2::{Digest,Sha256};
use std::{fs::File,io,os::unix::fs::{FileExt,MetadataExt}};

pub(super) const PROFILE:&[u8]=include_bytes!("../../../../../resources/samba-domain/initial-live.conf");
#[derive(Copy,Clone,PartialEq,Eq)]
struct Stamp{entity:Entity,mount:u64,mode:u32,uid:u32,links:u64,len:u64,mtime:(i64,i64),ctime:(i64,i64)}
impl Stamp{
    fn on(f:&File)->io::Result<Self>{let m=f.metadata()?;Ok(Self{entity:Entity::of(&m)?,
        mount:Mount::on(f)?.ok_or_else(||bad("missing installed live configuration mount"))?.id,
        mode:m.mode(),uid:m.uid(),links:m.nlink(),len:m.len(),
        mtime:(m.mtime(),m.mtime_nsec()),ctime:(m.ctime(),m.ctime_nsec())})}
    fn acceptable(&self,parent:&Stamp)->bool{
        self.mode==0o100444&&self.uid==0&&self.links==1&&self.len==PROFILE.len() as u64
            &&self.mount==parent.mount&&self.entity.dev==parent.entity.dev&&self.entity!=parent.entity
    }
}
fn source_at(parent:&File)->io::Result<File>{
    Ok(File::from(openat(parent,c"smb.conf",OFlags::PATH|OFlags::NOFOLLOW|OFlags::CLOEXEC,Mode::empty())?))
}
fn exact(bytes:&File)->io::Result<()>{
    let mut seen=vec![0;PROFILE.len()];bytes.read_exact_at(&mut seen,0)?;
    if seen!=PROFILE{return Err(bad("installed live configuration is not the fixed first-install profile"));}
    let mut extra=[0];
    loop{match bytes.read_at(&mut extra,PROFILE.len() as u64){
        Ok(0)=>return Ok(()),Ok(_)=>return Err(bad("additional bytes after live configuration")),
        Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,Err(e)=>return Err(e),
    }}
}
pub(super) struct Live{fd:File,stamp:Stamp}
impl Live{
    pub(super) fn select(parent:&File,approved_sha:&[u8;32])->io::Result<Self>{
        // No "sign what is there": the proposal, compiled fixed bytes and
        // complete physical original must agree, before any Samba call.
        if approved_sha!=&<[u8;32]>::from(Sha256::digest(PROFILE)){return Err(bad("the original approval did not select the first-install live profile"));}
        let dir=Stamp::on(parent)?;let os=source_at(parent)?;let stamp=Stamp::on(&os)?;
        if !stamp.acceptable(&dir){return Err(bad("not the installed root-owned 0444 live configuration"));}
        let fd=crate::pinned::sam_live_reader(&os,stamp.mount,PROFILE.len() as u64)?;
        let live=Self{fd,stamp};live.require(parent)?;Ok(live)
    }
    pub(super) fn require(&self,parent:&File)->io::Result<()>{
        let check=||->io::Result<()>{
            if !self.stamp.acceptable(&Stamp::on(parent)?)||Stamp::on(&self.fd)?!=self.stamp||Stamp::on(&source_at(parent)?)?!=self.stamp{
                return Err(bad("dedicated original live configuration changed"));}
            Ok(())
        };
        check()?;exact(&self.fd)?;check()
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
    fn installed_record_has_one_link_root_owner_same_mount_and_exact_mode(){
        let parent=Stamp{entity:Entity{dev:42,ino:12},mount:8,mode:0o40755,uid:0,links:2,len:1024,mtime:(1,0),ctime:(1,0)};
        let valid=Stamp{entity:Entity{dev:42,ino:14},mode:0o100444,links:1,len:PROFILE.len() as u64,..parent};
        assert!(valid.acceptable(&parent));
        for changed in [Stamp{uid:1000,..valid},Stamp{mode:0o100644,..valid},Stamp{links:2,..valid},Stamp{mount:9,..valid},Stamp{entity:parent.entity,..valid}]{
            assert!(!changed.acceptable(&parent));
        }
    }
}
