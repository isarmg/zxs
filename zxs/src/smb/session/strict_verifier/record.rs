//! Rust-only fixed private verifier record. Decoding is NOT qualification.
//! The protected load below is distinct from raw decoding and has no
//! authority to construct a live session.
use super::{ProtectedRevision,Rejected,Result,RevisionSource,Secret,VaultDecoder,challenge::utf16};
use crate::{smb::InheritedRun,system::reader::smb::runtime::QualifiedRustInstall};
use std::{num::NonZeroU64,sync::Arc};
use crate::smb::memory_erasure::Zeroizing;

use crate::system::shared_account::{VAULT_LENGTH as RECORD_BYTES,VAULT_MAGIC as MAGIC};
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
/// An exact-size input is required; only the original-FD load below binds
/// this decoder to the actual checked installation and current revision.
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
        for (id,b) in ids.iter_mut().zip(bytes[24..152].as_chunks::<16>().0) {
            if *b==[0;16] {return Err(Rejected);}
            *id = *b;
        }
        let [pool,init,setup,instance,binding,account,credential,record]=ids;
        let g=u64::from_be_bytes(bytes[152..160].try_into().map_err(|_|Rejected)?);
        let generation=NonZeroU64::new(g).filter(|n|n.get()<=i64::MAX as u64).ok_or(Rejected)?;
        let original=[pool,init,setup,instance,binding];
        if original.iter().enumerate().any(|(i,id)|original[..i].contains(id)) ||
            (if g==1{credential!=setup}else{original.contains(&credential)}) || bytes[224..240]==[0;16]{
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

pub(super) fn load_original(start:&Arc<InheritedRun>,actual:&QualifiedRustInstall)->Result<ProtectedRevision>{
    let buffer=start.read_for_decoder(&VaultDecoder(()),actual).map_err(|_|Rejected)?;
    let DecodedRecord{fields:q,key}=DecodedRecord::decode(buffer)?;
    let name=std::str::from_utf8(&q.login[..usize::from(q.login_len)]).map_err(|_|Rejected)?;
    let ids=[q.pool,q.init,q.setup,q.instance,q.binding,q.account,q.credential,q.record];
    if !actual.is_original_start(start)||
        !actual.matches_record_coordinates(&ids,q.generation.get() as i64,name){return Err(Rejected);}
    let host=actual.hostname().as_bytes();
    let mut spn=Vec::new();
    spn.try_reserve_exact(5+host.len()).map_err(|_|Rejected)?;
    spn.extend_from_slice(b"cifs/");spn.extend_from_slice(host);
    let result=ProtectedRevision{user:utf16(name.as_bytes())?,domain:utf16(DOMAIN.as_bytes())?,
        spn:utf16(&spn)?,epoch:q.generation,response_key:key,source:RevisionSource::Installed(Arc::downgrade(start))};
    actual.require_current().map_err(|_|Rejected)?;
    Ok(result)
}

#[cfg(test)]
#[path="../../../../tests/smb/credential_record.rs"]
mod tests;
