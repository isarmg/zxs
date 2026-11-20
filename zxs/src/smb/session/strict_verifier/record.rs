//! Rust-only fixed private verifier record. Decoding is NOT qualification.
//! Nothing here opens a file, qualifies a deployment or constructs a session.
use super::{Rejected,Result,Secret};
use std::num::NonZeroU64;
use zeroize::Zeroizing;

pub(super) const RECORD_BYTES:usize=240;
const MAGIC:&[u8;16]=b"ZXS-RUST-SMB-V1\0";
pub(super) const DOMAIN:&str="ZXSLOCAL";

/// Comparison data only. No public constructor or interpretation as a grant.
#[derive(Eq,PartialEq)]
pub(super) struct Coordinates {
    pool:[u8;16],
    init:[u8;16],
    setup:[u8;16],
    instance:[u8;16],
    binding:[u8;16],
    account:[u8;16],
    credential:[u8;16],
    record:[u8;16],
    generation:NonZeroU64,
    login_len:u8,
    login:[u8;64],
}

/// No Debug, Clone, raw key exposure or conversion to an active revision.
/// An exact-size input is required; the future original-FD reader must still
/// prove the object and EOF, and the owner must separately qualify ACTIVE.
#[must_use]
pub(super) struct DecodedRecord { fields:Coordinates, key:Secret }
impl DecodedRecord {
    pub(super) fn decode(bytes:Zeroizing<[u8;RECORD_BYTES]>)->Result<Self> {
        if &bytes[..16]!=MAGIC || bytes[16]!=1 || bytes[18..24]!=[0;6] {return Err(Rejected);}
        let n=usize::from(bytes[17]);
        if !(1..=64).contains(&n) {return Err(Rejected);}
        let login:&[u8]=&bytes[160..160+n];
        let name=crate::password::login_name(login).map_err(|_|Rejected)?;
        if matches!(name,"guest"|"anonymous"|"root"|"nobody") || bytes[160+n..224].iter().any(|&b|b!=0) {
            return Err(Rejected);
        }
        let mut ids=[[0u8;16];8];
        for (id,b) in ids.iter_mut().zip(bytes[24..152].chunks_exact(16)) {
            if b==[0;16] {return Err(Rejected);}
            id.copy_from_slice(b);
        }
        let [pool,init,setup,instance,binding,account,credential,record]=ids;
        let g=u64::from_be_bytes(bytes[152..160].try_into().map_err(|_|Rejected)?);
        let generation=NonZeroU64::new(g).filter(|n|n.get()<=i64::MAX as u64).ok_or(Rejected)?;
        if init==setup || credential==init || (credential==setup)!=(g==1) || bytes[224..240]==[0;16] {
            return Err(Rejected);
        }
        let fields=Coordinates{pool,init,setup,instance,binding,account,credential,record,generation,
            login_len:bytes[17],login:bytes[160..224].try_into().map_err(|_|Rejected)?};
        // Do not materialize an ordinary key array. Keep both owned buffers
        // zeroized on drop; do not claim this erases every compiler or OS copy.
        let mut key=Zeroizing::new([0u8;16]);key[..].copy_from_slice(&bytes[224..240]);
        drop(bytes);
        Ok(Self{fields,key})
    }
    pub(super) fn coordinates(&self)->&Coordinates {&self.fields}
}

#[cfg(test)]
#[path="../../../test/smb/credential_record.rs"]
mod tests;
