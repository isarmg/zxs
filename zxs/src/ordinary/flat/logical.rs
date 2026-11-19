//! Portable logical content, independent of material identity or physical
//! representation. A commitment neither grants read access nor certifies that
//! a destination was independently read back. No flat SHA reinterpretation.
use super::blocks::PER_PAGE;
use crate::{fs::invalid, hash::Sha256, ordinary::content::BLOCK};
use std::io::{self, Write};

const DOMAIN: &[u8] = b"ZXS-PORTABLE-1/LOGICAL-BLOCKS\0";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ordinary) struct LogicalV1 {
    length: u64,
    digest: [u8;32],
}
impl LogicalV1 {
    /// Supplied by the independently confirmed original node. Parsing the
    /// two fields alone is not a decision or a statement about current data.
    pub(in crate::ordinary) fn declared(length:u64, digest:[u8;32]) -> io::Result<Self> {
        if length > i64::MAX as u64 { return Err(invalid("logical size exceeds native offsets")); }
        Ok(Self{length,digest})
    }
    pub(in crate::ordinary) fn length(self)->u64 { self.length }
    pub(in crate::ordinary) fn digest(self)->[u8;32] { self.digest }

    fn start(length:u64)->io::Result<Sha256> {
        if length > i64::MAX as u64 { return Err(invalid("logical size exceeds native offsets")); }
        let mut h=Sha256::new(); h.update(DOMAIN)?; h.update(&length.to_be_bytes())?;
        h.update(&(BLOCK as u32).to_be_bytes())?; h.update(&(PER_PAGE as u32).to_be_bytes())?;
        Ok(h)
    }

    /// Private format calculation. At the physical boundary the caller has
    /// already verified THESE page digests, not just a purported hash of a
    /// directory, against one full material's own issue and checksum header.
    pub(super) fn from_pages(length:u64, pages:&[u8])->io::Result<Self> {
        let bytes=length.div_ceil(BLOCK).div_ceil(PER_PAGE).checked_mul(32)
            .ok_or_else(|| invalid("logical directory length overflow"))?;
        if u64::try_from(pages.len()).ok()!=Some(bytes) { return Err(invalid("wrong logical checksum page count")); }
        let mut h=Self::start(length)?; h.update(pages)?;
        Ok(Self{length,digest:h.finish()})
    }
}

/// Used only by an explicit complete stream/verification, not by each write.
/// Constant space: exact final block and page, never capacity padding bytes.
pub(super) struct Whole {
    root:Sha256, page:Sha256, block:Sha256,
    length:u64, seen:u64, in_block:u64, in_page:u64,
}
impl Whole {
    pub(super) fn new(length:u64)->io::Result<Self> {
        Ok(Self{root:LogicalV1::start(length)?,page:Sha256::new(),block:Sha256::new(),
            length,seen:0,in_block:0,in_page:0})
    }
    fn close_page(&mut self)->io::Result<()> {
        let digest=std::mem::replace(&mut self.page,Sha256::new()).finish();
        self.root.update(&digest)?; self.in_page=0; Ok(())
    }
    fn close_block(&mut self)->io::Result<()> {
        let digest=std::mem::replace(&mut self.block,Sha256::new()).finish();
        self.page.update(&digest)?; self.in_block=0; self.in_page+=1;
        if self.in_page==PER_PAGE { self.close_page()?; }
        Ok(())
    }
    pub(super) fn finish(mut self)->io::Result<LogicalV1> {
        if self.seen!=self.length { return Err(invalid("incomplete logical verification stream")); }
        if self.in_block!=0 { self.close_block()?; }
        if self.in_page!=0 { self.close_page()?; }
        Ok(LogicalV1{length:self.length,digest:self.root.finish()})
    }
}
impl Write for Whole {
    fn write(&mut self, mut bytes:&[u8])->io::Result<usize> {
        let n=bytes.len(); let count=u64::try_from(n).map_err(|_| invalid("oversized logical buffer"))?;
        let next=self.seen.checked_add(count).filter(|n| *n<=self.length)
            .ok_or_else(|| invalid("too many bytes for the original logical size"))?;
        while !bytes.is_empty() {
            let k=bytes.len().min((BLOCK-self.in_block) as usize);
            self.block.update(&bytes[..k])?; bytes=&bytes[k..]; self.in_block+=k as u64;
            if self.in_block==BLOCK { self.close_block()?; }
        }
        self.seen=next; Ok(n)
    }
    fn flush(&mut self)->io::Result<()> { Ok(()) }
}
