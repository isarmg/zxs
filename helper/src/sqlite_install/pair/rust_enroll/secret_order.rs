//! Public exact scope of a prepared low writer; it cannot include a password.
use super::{bad,manifest::{self,Intent,Manifest},receipt::Observed};
use crate::sqlite_install::pair::native_sql::rust_vault::Desired;
use sha2::{Digest,Sha256};
use std::{fs::File,io};

const DOMAIN:&[u8;16]=b"ZXS-RUST-SEAL-1\0";
const LIMIT:usize=24*1024;
fn no()->io::Error{bad("wrong original Rust vault writer order")}
pub(in crate::sqlite_install::pair) struct SecretOrder{
    pub(in crate::sqlite_install::pair) manifest:Manifest,
    pub(in crate::sqlite_install::pair) selected:Observed,
}
impl SecretOrder {
    pub(in crate::sqlite_install::pair) fn encode(m:&Manifest,s:&Observed)->io::Result<Vec<u8>>{
        let raw=m.encode()?;let n=u16::try_from(raw.len()).map_err(|_|no())?;
        let mut exact=Vec::with_capacity(LIMIT);
        exact.extend(DOMAIN);exact.extend(n.to_be_bytes());exact.extend(raw);exact.extend(s.encode()?);
        if exact.len()>LIMIT{return Err(no());}Ok(exact)
    }
    pub(in crate::sqlite_install::pair) fn read(sealed:&File)->io::Result<(Self,[u8;32])>{
        let raw=manifest::seal_bytes(sealed,LIMIT)?;let digest=scope(&raw);
        let f=raw.strip_prefix(DOMAIN).ok_or_else(no)?;
        let (n,values)=f.split_at_checked(2).ok_or_else(no)?;
        let n=usize::from(u16::from_be_bytes(n.try_into().map_err(|_|no())?));
        let (m,preview)=values.split_at_checked(n).ok_or_else(no)?;
        let manifest=manifest::decode(m)?;
        let selected=Observed::decode(preview,&manifest.plan)?;
        if manifest.intent!=Intent::Fresh||selected.state!=1||selected.target.op!=manifest.order.op{return Err(no());}
        let matches=match(&manifest.order.role,&selected.previous){
            (Desired::Setup{smb,..},None)=>smb==&selected.target.login,
            (Desired::Reset{previous,generation},Some(old))=>*previous==old.op&&*generation==old.generation,
            _=>false,
        };
        if !matches{return Err(no());}
        Ok((Self{manifest,selected},digest))
    }
}
pub(in crate::sqlite_install::pair) fn scope(raw:&[u8])->[u8;32]{
    let mut h=Sha256::new();h.update(b"ZXS-HELPER/RUST-SMB-SEALED-SLOT-1\0");h.update(raw);h.finalize().into()
}
