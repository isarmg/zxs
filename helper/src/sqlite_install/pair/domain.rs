//! Public, nonsecret SAM material only. A parse cannot issue accounts, start
//! smbd, authorize provisioning or prove that a running smbd used this domain.
mod handoff;
mod provision;
use crate::sqlite_install::{bad,request::Id};
use std::io;

const NEW:&[u8]=b"ZXS-SAM/NEW-1\0";
const BOUND:&[u8]=b"ZXS-SAM/BOUND-1\0";
const NAME:&[u8]=b"ZXSLOCAL";

#[derive(Eq,PartialEq)]
pub(super) struct LocalDomain([u8;24]);

impl LocalDomain {
    fn from_name_and_domain(row:&[u8])->io::Result<Self>{
        let v:&[u8]=row.strip_prefix(NAME).ok_or_else(||bad("the fixed local SAM name does not match"))?;
        let sid:[u8;24]=v.try_into().map_err(|_|bad("expected one 24-byte local domain SID, not a user or account"))?;
        if sid[..12]!=[1,4,0,0,0,0,0,5,21,0,0,0]||sid[12..]==[0;12]{return Err(bad("invalid standalone local SAM domain"));}
        Ok(Self(sid))
    }
    /// Used only after separate launcher/normal exit/EOF/originals checks.
    pub(super) fn guest_bytes(b:&[u8])->io::Result<Self>{
        Self::from_name_and_domain(b.strip_prefix(NEW).ok_or_else(||bad("not the fresh SAM guest result"))?)
    }
    /// Match bytes against independently selected installation fields. Never
    /// adopt identifiers or select paths from the disk record or the guest.
    pub(super) fn expected_binding(b:&[u8],pool:&Id,instance:&Id,op:&Id,live_conf:&[u8;32])->io::Result<Self>{
        if [pool,instance,op].iter().any(|id|**id==[0;16]){return Err(bad("missing independent installation identifiers"));}
        let v=b.strip_prefix(BOUND).ok_or_else(||bad("not the SAM origin record"))?;
        if v.len()!=80+NAME.len()+24||v[..16]!=*pool||v[16..32]!=*instance||v[32..48]!=*op||v[48..80]!=*live_conf{
            return Err(bad("not the selected pool, dedicated instance, original operation and live configuration"));}
        Self::from_name_and_domain(&v[80..])
    }
    fn encode_binding(&self,pool:&Id,instance:&Id,op:&Id,live:&[u8;32])->io::Result<[u8;128]>{
        let mut b=[0;128];b[..16].copy_from_slice(BOUND);b[16..32].copy_from_slice(pool);
        b[32..48].copy_from_slice(instance);b[48..64].copy_from_slice(op);b[64..96].copy_from_slice(live);
        b[96..104].copy_from_slice(NAME);b[104..].copy_from_slice(&self.0);
        if Self::expected_binding(&b,pool,instance,op,live)?!=*self{return Err(bad("wrong original instance binding"));}
        Ok(b)
    }
    pub(super) fn expected_smb_sid(&self)->[u8;28]{
        let mut result=[0;28];result[..24].copy_from_slice(&self.0);result[1]=5;
        result[24..].copy_from_slice(&1000u32.to_le_bytes());result
    }
}

#[cfg(test)]
#[path="../../../tests/sqlite/samba_origin.rs"]
mod tests;
