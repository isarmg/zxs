//! Serialize exactly the public START the existing independently qualified
//! child decodes. Never make a grant or supply a second startup syntax.
use super::{ENV_MAGIC,ENV_MAX,Fault,MAX_PATH,MAX_PLAN,Result,START_BYTES,START_MAGIC,Start,VaultStamp};
use super::super::cohort::PairOffer;
use crate::{Entity,system::reader::smb::runtime::PreflightEpoch};

pub(in crate::smb::launch) struct Scope<'a>{
    pub(in crate::smb::launch) nonce:[u8;32],
    pub(in crate::smb::launch) release:[u8;32],
    pub(in crate::smb::launch) subject:(u32,u32),
    pub(in crate::smb::launch) system:Entity,
    pub(in crate::smb::launch) path:&'a str,
    pub(in crate::smb::launch) plan:&'a [u8],
    pub(in crate::smb::launch) listener_cookie:u64,
    pub(in crate::smb::launch) vault:VaultStamp,
    pub(in crate::smb::launch) epoch:&'a PreflightEpoch,
    pub(in crate::smb::launch) pair:PairOffer,
}
pub(in crate::smb::launch) fn encode(s:Scope<'_>)->Result<([u8;START_BYTES],Vec<u8>)>{
    if s.path.len()>MAX_PATH||s.plan.len()>MAX_PLAN||s.epoch.login.len()>64{
        return Err(Fault::Shape);
    }
    let mut out=Vec::new();out.try_reserve_exact(ENV_MAX).map_err(|_|Fault::Resources)?;
    out.extend_from_slice(ENV_MAGIC);out.extend_from_slice(&s.nonce);out.extend_from_slice(&s.release);
    out.extend_from_slice(&s.subject.0.to_be_bytes());out.extend_from_slice(&s.subject.1.to_be_bytes());
    out.extend_from_slice(&s.system.device.to_be_bytes());out.extend_from_slice(&s.system.inode.to_be_bytes());
    out.extend_from_slice(&(s.path.len() as u16).to_be_bytes());out.extend_from_slice(s.path.as_bytes());
    out.extend_from_slice(&(s.plan.len() as u16).to_be_bytes());out.extend_from_slice(s.plan);
    out.push(1);
    out.extend_from_slice(&s.listener_cookie.to_be_bytes());
    for id in [s.epoch.account,s.epoch.op,s.epoch.record]{out.extend_from_slice(&id);}
    for word in [s.vault.dev,s.vault.ino]{out.extend_from_slice(&word.to_be_bytes());}
    out.extend_from_slice(&s.vault.ctime_s.to_be_bytes());out.extend_from_slice(&s.vault.ctime_ns.to_be_bytes());
    out.extend_from_slice(&s.vault.mtime_s.to_be_bytes());out.extend_from_slice(&s.vault.mtime_ns.to_be_bytes());
    out.extend_from_slice(&s.epoch.sid);out.extend_from_slice(&s.epoch.generation.to_be_bytes());
    out.push(s.epoch.login.len() as u8);out.extend_from_slice(s.epoch.login.as_bytes());
    out.extend_from_slice(&s.pair.encode());
    let digest=crate::hash::digest(&out).map_err(|_|Fault::Shape)?;
    let mut frame=[0;START_BYTES];
    frame[..16].copy_from_slice(START_MAGIC);frame[16..48].copy_from_slice(&s.nonce);frame[48..].copy_from_slice(&digest);
    let _=Start::decode(&frame)?.verify(&out)?; // same strict child format, before the first send
    Ok((frame,out))
}
