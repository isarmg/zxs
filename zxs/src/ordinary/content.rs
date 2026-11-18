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

/// Receive an exact forward-only stream into an already preallocated, private
/// original inode. The caller owns the actual data/link offsets and header.
/// No externally named material or authority is changed here.
pub(super) fn stage(claim:DraftClaim,mut input:impl std::io::Read,file:&std::fs::File,
    at:impl Fn(u64,usize)->(u64,u64))->io::Result<[u8;32]>{
    use crate::fs::stream::{read_at,write_at};
    let mut h=crate::hash::Sha256::new();let mut buf=vec![0;claim.bytes.clamp(1,BLOCK) as usize];
    for i in 0..chunks(claim.bytes){
        let n=(claim.bytes-i*BLOCK).min(BLOCK) as usize;let (data,index)=at(i,n);let part=&mut buf[..n];
        input.read_exact(part)?;h.update(part)?;write_at(file,part,data)?;write_at(file,&crate::hash::digest(part)?,index)?;
    }
    loop{match input.read(&mut buf[..1]){
        Ok(0)=>break,Ok(_)=>return Err(invalid("trailing bytes after the declared extent")),
        Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,Err(e)=>return Err(e),
    }}
    if h.finish()!=claim.sha256{return Err(invalid("input extent checksum differs"));}
    drop(buf);let mut head=end(claim)?;
    for i in (0..chunks(claim.bytes)).rev(){
        let n=(claim.bytes-i*BLOCK).min(BLOCK) as usize;let (_,index)=at(i,n);let mut digest=[0;32];
        read_at(file,&mut digest,index)?;write_at(file,&head,index)?;head=link(i,n,digest,head)?;
    }
    Ok(head)
}
