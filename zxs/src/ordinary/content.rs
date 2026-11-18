//! The original saved-byte chain is a content commitment, not a locator.
//! A formal raw FULL carries its *own* links in the native FILE node record;
//! its reader never opens the working file to verify or substitute a byte.
use super::draft::types::{DraftClaim,DRAFT_CHUNK};
use crate::fs::invalid;
use std::io;
pub(super) const BLOCK:u64=DRAFT_CHUNK as u64;
pub(super) fn chunks(bytes:u64)->u64{bytes/BLOCK+u64::from(bytes%BLOCK!=0)}
pub(super) fn end(claim:DraftClaim)->io::Result<[u8;32]>{
    let mut h=crate::hash::Sha256::new();h.update(b"ZXS/WORKING/END\0")?;h.update(&claim.bytes.to_be_bytes())?;h.update(&claim.sha256)?;Ok(h.finish())
}
pub(super) fn link(i:u64,n:usize,digest:[u8;32],next:[u8;32])->io::Result<[u8;32]>{
    let mut h=crate::hash::Sha256::new();h.update(b"ZXS/WORKING/CHUNK\0")?;h.update(&i.to_be_bytes())?;
    h.update(&(n as u64).to_be_bytes())?;h.update(&digest)?;h.update(&next)?;Ok(h.finish())
}
pub(super) fn visit(claim:DraftClaim,head:[u8;32],
    mut read:impl FnMut(u64,&mut[u8],&mut[u8;32])->io::Result<()>,
    mut current:impl FnMut()->io::Result<()>,
    mut output:impl FnMut(u64,&[u8],[u8;32])->io::Result<()>)->io::Result<()>{
    let mut expected=head;let mut content=crate::hash::Sha256::new();let mut buf=vec![0;claim.bytes.clamp(1,BLOCK) as usize];
    for i in 0..chunks(claim.bytes){
        let n=(claim.bytes-i*BLOCK).min(BLOCK) as usize;let part=&mut buf[..n];let mut next=[0;32];
        read(i,part,&mut next)?;
        if link(i,n,crate::hash::digest(part)?,next)?!=expected{return Err(invalid("FILE content chunk differs from its original commitment"));}
        current()?;content.update(part)?;output(i,part,next)?;expected=next;
    }
    if expected!=end(claim)?||content.finish()!=claim.sha256{return Err(invalid("FILE length, content or terminal commitment differs"));}
    current()?;Ok(())
}
