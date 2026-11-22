//! Bounded framing of the private supervisor's public START payload.
//! These values do not prove the supervisor or grant use of the vault.
use super::{Fault,Result,cohort::{self,PairOffer,WIRE_BYTES}};
use crate::system::reader::smb::runtime::PLAN_MAX_BYTES;
use std::path::Path;
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
pub(super) mod send;

const START_MAGIC:&[u8;16]=b"ZXS-RUST/START2\0";
const READY_MAGIC:&[u8;16]=b"ZXS-RUST/PREP02\0";
const ENV_MAGIC:&[u8;19]=b"ZXS-RUST-SMB-ENV-2\0";
pub(super) const START_BYTES:usize=80;
const MAX_PATH:usize=4096;
const MAX_PLAN:usize=PLAN_MAX_BYTES;
pub(super) const ENV_MAX:usize=ENV_MAGIC.len()+32+32+4+4+8+8+2+MAX_PATH+2+MAX_PLAN+
    1+8+16*3+40+28+8+1+64+WIRE_BYTES;

#[must_use]
pub(super) struct Start{nonce:[u8;32],digest:[u8;32]}
impl Start {
    pub(super) fn decode(raw:&[u8])->Result<Self>{
        if raw.len()!=START_BYTES{return Err(Fault::Shape);}
        let mut c=Cursor(raw.strip_prefix(START_MAGIC).ok_or(Fault::Shape)?);
        let nonce=c.array()?;let digest=c.array()?;
        if nonce==[0;32]||digest==[0;32]{return Err(Fault::Shape);}
        Ok(Self{nonce,digest})
    }
    pub(super) fn verify(&self,raw:&[u8])->Result<Envelope>{
        if raw.len()>ENV_MAX||crate::hash::digest(raw).map_err(|_|Fault::Shape)?!=self.digest{
            return Err(Fault::Shape);
        }
        let env=Envelope::decode(raw)?;
        if env.nonce!=self.nonce{return Err(Fault::Shape);}
        Ok(env)
    }
    // Public acknowledgement bytes, not a readiness or sending capability.
    pub(super) fn prepared_frame(&self)->[u8;START_BYTES]{
        let mut body=[0;START_BYTES];body[..16].copy_from_slice(READY_MAGIC);
        body[16..48].copy_from_slice(&self.nonce);body[48..].copy_from_slice(&self.digest);body
    }
}

type Id=[u8;16];
#[must_use]
pub(super) struct Envelope {
    nonce:[u8;32],
    pub(super) release_sha:[u8;32],
    pub(super) uid:u32,pub(super) gid:u32,
    pub(super) sdev:u64,pub(super) sino:u64,
    pub(super) system:String,pub(super) plan:Vec<u8>,
    pub(super) listener_cookie:u64,
    pub(super) account:Id,pub(super) credential:Id,pub(super) record:Id,
    pub(super) vault:VaultStamp,
    pub(super) sid:[u8;28],pub(super) generation:i64,pub(super) login:String,
    pub(super) pair:PairOffer,
}
/// Public stat witness, not proof of credential contents or authorization.
#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) struct VaultStamp {
    pub(super) dev:u64,pub(super) ino:u64,
    pub(super) ctime_s:i64,pub(super) ctime_ns:u32,
    pub(super) mtime_s:i64,pub(super) mtime_ns:u32,
}
impl VaultStamp{
    fn decode(c:&mut Cursor<'_>)->Result<Self>{
        let s=Self{dev:c.u64()?,ino:c.u64()?,ctime_s:c.i64()?,ctime_ns:c.u32()?,
            mtime_s:c.i64()?,mtime_ns:c.u32()?};
        if s.ino==0||s.ctime_ns>=1_000_000_000||s.mtime_ns>=1_000_000_000{return Err(Fault::Shape);}
        Ok(s)
    }
}
impl Envelope{
    fn decode(raw:&[u8])->Result<Self>{
        let mut c=Cursor(raw.strip_prefix(ENV_MAGIC).ok_or(Fault::Shape)?);
        let nonce=c.array()?;let release_sha=c.array()?;
        let uid=c.u32()?;let gid=c.u32()?;let sdev=c.u64()?;let sino=c.u64()?;
        if nonce==[0;32]||release_sha==[0;32]||sino==0||matches!(uid,0|65534|u32::MAX)||matches!(gid,0|65534|u32::MAX){
            return Err(Fault::Shape);
        }
        let system=std::str::from_utf8(c.counted(MAX_PATH)?).map_err(|_|Fault::Shape)?;
        crate::fs::check_path(Path::new(system)).map_err(|_|Fault::Shape)?;
        let public_plan=c.counted(MAX_PLAN)?; // Core qualifies its actual plan and SQL.
        if c.byte()?!=1{return Err(Fault::Shape);} // same START layout, exactly one listener
        let listener_cookie=c.u64()?;
        if listener_cookie==0{return Err(Fault::Shape);}
        let account=c.id()?;let credential=c.id()?;let record=c.id()?;
        let vault=VaultStamp::decode(&mut c)?;
        let sid=c.array()?; // A raw SID here is not a session principal.
        let generation=c.i64()?;
        let login_len=usize::from(c.byte()?);
        if generation<=0||!(1..=64).contains(&login_len){return Err(Fault::Shape);}
        let login=crate::password::login_name(c.take(login_len)?).map_err(|_|Fault::Shape)?;
        let pair=PairOffer::decode(c.array()?)?;
        if matches!(login,"root"|"nobody"|"anonymous"|"guest")||!c.0.is_empty()||nonce==pair.nonce{return Err(Fault::Shape);}
        let epoch=crate::system::reader::smb::runtime::PreflightEpoch{account,op:credential,record,sid,generation,login:text(login)?};
        let plan_sha=crate::hash::digest(public_plan).map_err(|_|Fault::Shape)?;
        if pair.cohort!=cohort::digest(release_sha,plan_sha,crate::Entity{device:sdev,inode:sino},&epoch)?{
            return Err(Fault::Handoff);
        }
        let mut plan=Vec::new();plan.try_reserve_exact(public_plan.len()).map_err(|_|Fault::Resources)?;
        plan.extend_from_slice(public_plan);
        Ok(Self{nonce,release_sha,uid,gid,sdev,sino,system:text(system)?,plan,listener_cookie,
            account,credential,record,vault,sid,generation,login:epoch.login,pair})
    }
}
fn text(s:&str)->Result<String>{
    let mut out=String::new();out.try_reserve_exact(s.len()).map_err(|_|Fault::Resources)?;
    out.push_str(s);Ok(out)
}
struct Cursor<'a>(&'a[u8]);
impl<'a> Cursor<'a>{
    fn take(&mut self,n:usize)->Result<&'a[u8]>{
        let (x,t)=self.0.split_at_checked(n).ok_or(Fault::Shape)?;self.0=t;Ok(x)
    }
    fn array<const N:usize>(&mut self)->Result<[u8;N]>{
        self.take(N)?.try_into().map_err(|_|Fault::Shape)
    }
    fn byte(&mut self)->Result<u8>{let [v]=self.array()?;Ok(v)}
    fn u32(&mut self)->Result<u32>{Ok(u32::from_be_bytes(self.array()?))}
    fn u64(&mut self)->Result<u64>{Ok(u64::from_be_bytes(self.array()?))}
    fn i64(&mut self)->Result<i64>{Ok(i64::from_be_bytes(self.array()?))}
    fn id(&mut self)->Result<Id>{let v=self.array()?;if v==[0;16]{Err(Fault::Shape)}else{Ok(v)}}
    fn counted(&mut self,max:usize)->Result<&'a[u8]>{
        let n=usize::from(u16::from_be_bytes(self.array()?));
        if n==0||n>max{return Err(Fault::Shape);}
        self.take(n)
    }
}

#[cfg(test)]
#[path="../../../tests/smb/rust_launch_envelope.rs"]
mod tests;
