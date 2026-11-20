//! A block of immutable bytes inside a regular file that can grow past it.
//! The expected digest and physical boundaries always come from the selected
//! immutable checksum entry or the exact reserved work span, never from EOF.
use super::{File,invalid};
use std::{io,os::unix::fs::FileExt};

/// This is a per-verified-read memory bound, not a FILE or logical offset limit.
const BLOCK:u64=crate::DRAFT_CHUNK as u64;
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) struct SealedExtent{start:u64,count:u32,sha:[u8;32]}
impl SealedExtent{
    pub(crate) fn original(start:u64,count:usize,sha:[u8;32])->io::Result<Self>{
        if count==0||count as u64>BLOCK||start.checked_add(count as u64).is_none_or(|v|v>i64::MAX as u64){
            return Err(invalid("invalid physical checksum extent"));}
        Ok(Self{start,count:count as u32,sha})
    }
    pub(crate) fn start(self)->u64{self.start}
    pub(crate) fn end(self)->u64{self.start+u64::from(self.count)}
    /// No prefix leaves this function on a checksum mismatch, including a
    /// short, originally unaligned last block in a file that now has a tail.
    /// Object/name identity and authorization remain the caller's obligation.
    pub(crate) fn checked<'a>(self,file:&File,buffer:&'a mut [u8])->io::Result<&'a [u8]>{
        let b=buffer.get_mut(..self.count as usize).ok_or_else(||invalid("sealed extent exceeds this read's buffer"))?;
        file.read_exact_at(b,self.start)?;
        if crate::hash::digest(b)?!=self.sha{return Err(invalid("physical extent differs from its original checksum"));}
        Ok(b)
    }
}
