//! Deliver a native SMB read only through a private pending reply, then
//! recheck the selected reader's exact source before acknowledging it.
use super::{InitialReader,check};
use crate::{fs::invalid,ordinary::working_round::Used};
use std::io::{self,Write};

pub(super) fn data_version(source:&InitialReader)->io::Result<i64>{
    let q=source.conn.prepare(c"PRAGMA main.data_version")?;
    match q.step(){100=>{},rc=>{check(rc)?;return Err(invalid("cannot observe SQLite data version"));}}
    let n=q.integer(0)?;q.done()?;Ok(n)
}
pub(super) struct Buffer{data:Vec<u8>,limit:usize}
impl Write for Buffer{
    fn write(&mut self,bytes:&[u8])->io::Result<usize>{
        if bytes.len()>self.limit-self.data.len(){return Err(invalid("oversized checked SMB read window"));}
        if !bytes.is_empty()&&self.data.capacity()<self.limit{
            self.data.try_reserve_exact(self.limit-self.data.len()).map_err(|_|io::Error::other("cannot reserve a complete SMB read window"))?;
        }
        self.data.extend_from_slice(bytes);Ok(bytes.len())
    }
    fn flush(&mut self)->io::Result<()>{Ok(())}
}
fn collect(offset:u64,count:u32,size:u64,range:impl FnOnce(u64,u64,&mut Buffer)->io::Result<Used>)
    ->io::Result<(Vec<u8>,Used)>{
    // Pinned upstream absolute input maximum, not a new negotiated size.
    if count>0xffff00||offset.checked_add(u64::from(count)).is_none_or(|end|end>i64::MAX as u64){
        return Err(invalid("invalid native SMB read request"));
    }
    let start=offset.min(size);let n=(size-start).min(u64::from(count)) as usize;
    let mut buffer=Buffer{data:Vec::new(),limit:n};
    let used=range(start,n as u64,&mut buffer)?;
    if buffer.data.len()!=n{return Err(invalid("incomplete checked SMB read window"));}
    Ok((buffer.data,used))
}
pub(super) fn deliver<T>(offset:u64,count:u32,size:u64,range:impl FnOnce(u64,u64,&mut Buffer)->io::Result<Used>,
    consume:impl FnOnce(&[u8])->io::Result<T>,mut confirm:impl FnMut(&Used)->io::Result<()>)->io::Result<T>{
    let (data,used)=collect(offset,count,size,range)?;
    let result=consume(&data)?;
    crate::ordinary::after_output(&mut ||confirm(&used))?;
    Ok(result)
}
// Rust keeps (rather than transmits) the actual complete immutable window.
// Its original used-source proof travels with the bytes until late send.
pub(super) fn prepare(offset:u64,count:u32,size:u64,range:impl FnOnce(u64,u64,&mut Buffer)->io::Result<Used>,
    mut confirm:impl FnMut(&Used)->io::Result<()>)->io::Result<(Vec<u8>,Used)>{
    let (data,used)=collect(offset,count,size,range)?;
    crate::ordinary::after_output(&mut ||confirm(&used))?;
    Ok((data,used))
}
