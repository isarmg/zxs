//! Nonsecret bindings in the ZXS-owned site record. Parsing bytes, even with
//! a valid format, is not authorization to attach the site, storage or image.
use super::denied;
use std::{io,os::unix::ffi::OsStrExt,path::{Path,PathBuf}};

const MAGIC: &[u8;16] = b"ZXS-RUST-SITE-1\0";
const PLAN_MAGIC: &[u8;16] = b"ZXS-RUST-SMB-I1\0";
const MAX_PATH:usize=4095;
pub(super) const MAX_RECORD:usize=MAGIC.len()+16*6+4*2+32*2+2*(2+MAX_PATH);
pub(super) const MAX_PLAN:usize=17_885;
type Id=[u8;16];

#[derive(Clone,Copy,Eq,PartialEq)]
pub(in crate::smb::launch) struct Origin{
    pub(in crate::smb::launch) pool:Id,
    pub(in crate::smb::launch) init:Id,
    pub(in crate::smb::launch) instance:Id,
    pub(in crate::smb::launch) bind:Id,
    pub(in crate::smb::launch) setup:Id,
}
impl Origin{
    fn decode(c:&mut Cursor<'_>)->io::Result<Self>{
        let ids:[Id;5]=[c.take()?,c.take()?,c.take()?,c.take()?,c.take()?];
        if ids.contains(&[0;16])||ids.iter().enumerate().any(|(i,v)|ids[..i].contains(v)){
            return Err(denied());
        }
        let [pool,init,instance,bind,setup]=ids;
        Ok(Self{pool,init,instance,bind,setup})
    }
}
#[derive(Clone,Copy,Eq,PartialEq)]
pub(in crate::smb::launch) struct Subject{
    pub(in crate::smb::launch) init_actor:Id,
    pub(in crate::smb::launch) uid:u32,
    pub(in crate::smb::launch) gid:u32,
}

pub(super) struct Approval{
    pub(super) origin:Origin,
    pub(super) subject:Subject,
    pub(super) system:PathBuf,
    pub(super) image:PathBuf,
    pub(super) plan_sha:[u8;32],
    pub(super) image_sha:[u8;32],
}
impl Approval{
    pub(super) fn decode(raw:&[u8])->io::Result<Self>{
        let mut c=Cursor(raw.strip_prefix(MAGIC).filter(|_|raw.len()<=MAX_RECORD).ok_or_else(denied)?);
        let origin=Origin::decode(&mut c)?;
        let init_actor=c.take()?;
        let uid=u32::from_be_bytes(c.take()?);let gid=u32::from_be_bytes(c.take()?);
        let plan_sha=c.take()?;let image_sha=c.take()?;
        let system=c.path()?;let image=c.path()?;
        if init_actor==[0;16]||[uid,gid].iter().any(|v|matches!(*v,0|65534|u32::MAX))||
            plan_sha==[0;32]||image_sha==[0;32]||!c.0.is_empty()||
            image.starts_with(&system)||system.starts_with(&image){
            return Err(denied());
        }
        Ok(Self{origin,subject:Subject{init_actor,uid,gid},system,image,plan_sha,image_sha})
    }
    /// Check the fixed origin and whole approved plan hash. Only the low
    /// Core production reader interprets the plan and current shared ACTIVE rows.
    pub(super) fn require_plan(&self,raw:&[u8])->io::Result<()>{
        let mut c=Cursor(raw.strip_prefix(PLAN_MAGIC).filter(|_|raw.len()<=MAX_PLAN).ok_or_else(denied)?);
        if Origin::decode(&mut c)?!=self.origin||c.0.is_empty()||crate::hash::digest(raw)?!=self.plan_sha{
            return Err(denied());
        }
        Ok(())
    }
}
pub(super) fn check_path(path:&Path)->io::Result<()>{
    crate::fs::check_path(path)?;
    let s=path.to_str().ok_or_else(denied)?;
    if s.len()>MAX_PATH||s.chars().any(char::is_control)||
        path.components().any(|n|n.as_os_str().as_bytes().len()>255){
        return Err(denied());
    }
    Ok(())
}
struct Cursor<'a>(&'a[u8]);
impl Cursor<'_>{
    fn take<const N:usize>(&mut self)->io::Result<[u8;N]>{
        let (a,b)=self.0.split_at_checked(N).ok_or_else(denied)?;
        self.0=b;a.try_into().map_err(|_|denied())
    }
    fn path(&mut self)->io::Result<PathBuf>{
        let n=usize::from(u16::from_be_bytes(self.take()?));
        let (a,b)=self.0.split_at_checked(n).filter(|_|(1..=MAX_PATH).contains(&n)).ok_or_else(denied)?;
        self.0=b;
        let p=Path::new(std::str::from_utf8(a).map_err(|_|denied())?);
        check_path(p)?;Ok(p.to_path_buf())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/attached/smb/launch/site/approval/tests.rs"]
mod tests;
