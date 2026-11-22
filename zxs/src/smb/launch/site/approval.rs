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
    /// Core production reader interprets the plan and v18 shared ACTIVE rows.
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
mod tests{
    use super::*;
    fn public_plan()->Vec<u8>{
        // Immutable public plan, never a credential or real installation path.
        let hex=b"5a58532d525553542d534d422d49310011111111111111111111111111111111121212121212121212121212121212121616161616161616161616161616161617171717171717171717171717171717131313131313131313131313131313130b666978747572652e6c616e010418c00002110133333333333333333333333333333333020004446f63730006e59b9ee694b6";
        hex.as_chunks::<2>().0.iter().map(|n|u8::from_str_radix(std::str::from_utf8(n).unwrap(),16).unwrap()).collect()
    }
    fn record(system:&[u8],image:&[u8])->Vec<u8>{
        let mut raw=MAGIC.to_vec();
        raw.extend_from_slice(&public_plan()[16..96]);
        raw.extend([31;16]);raw.extend(1001u32.to_be_bytes());raw.extend(1002u32.to_be_bytes());
        raw.extend(crate::hash::digest(&public_plan()).unwrap());raw.extend([41;32]);
        for path in [system,image]{raw.extend((path.len() as u16).to_be_bytes());raw.extend(path);}
        raw
    }
    #[test]
    fn rust_site_record_binds_original_scope_and_the_whole_plan(){
        let b=record(b"/zxs/disks/system",b"/zxs/bin/zxs");let r=Approval::decode(&b).unwrap();
        assert_eq!(r.origin.instance,[0x16;16]);assert_eq!(r.origin.setup,[0x13;16]);
        assert_eq!((r.subject.uid,r.subject.gid),(1001,1002));
        assert_eq!(r.system.as_path(),Path::new("/zxs/disks/system"));assert_eq!(r.image_sha,[41;32]);
        let p=public_plan();r.require_plan(&p).unwrap();
        for i in [0,17,96,p.len()-1]{let mut other=p.clone();other[i]^=1;assert!(r.require_plan(&other).is_err());}
        for k in 0..b.len(){assert!(Approval::decode(&b[..k]).is_err());}
    }
    #[test]
    fn rust_site_record_does_not_authorize_aliases_unknown_principals_or_extras(){
        for s in [b"/".as_slice(),b"/mnt//system",b"/mnt/../system",b"/mnt/S\n",b"/\xff",b"/opt/zxs/bin/zxs/other"]{
            assert!(Approval::decode(&record(s,b"/opt/zxs/bin/zxs")).is_err());
        }
        let raw=record(b"/mnt/S",b"/opt/zxs/bin/zxs");
        let mut extra=raw.clone();extra.push(0);assert!(Approval::decode(&extra).is_err());
        for (start,len) in [(16,16),(96,16),(112,4),(116,4),(120,32),(152,32)]{
            let mut bad=raw.clone();bad[start..start+len].fill(0);assert!(Approval::decode(&bad).is_err());
        }
        let mut bad=raw;let id:[u8;16]=bad[16..32].try_into().unwrap();
        bad[32..48].copy_from_slice(&id);assert!(Approval::decode(&bad).is_err());
    }
}
