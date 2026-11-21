//! Checked original nonexecutable sealed bytes for private local workers.
use super::bad;
use rustix::fs::{SealFlags,fcntl_get_seals};
use std::{fs::File,io,os::unix::fs::{FileExt,MetadataExt}};

pub(super) fn read_sealed(f:&File,max:usize)->io::Result<Vec<u8>>{sealed_owner(f,max,(0,0))}
pub(super) fn sealed_owner(f:&File,max:usize,owner:(u32,u32))->io::Result<Vec<u8>>{
        let m=f.metadata()?;
        if !m.is_file()||(m.uid(),m.gid())!=owner||m.nlink()!=0||m.mode()&0o111!=0||m.len()==0||m.len()>max as u64
            ||!fcntl_get_seals(f)?.contains(SealFlags::SEAL|SealFlags::SHRINK|SealFlags::GROW|SealFlags::WRITE|SealFlags::EXEC){return Err(error());}
        let mut v=vec![0;m.len()as usize];f.read_exact_at(&mut v,0)?;
        loop{match f.read_at(&mut [0],m.len()){
            Ok(0)=>break,Err(e)if e.kind()==io::ErrorKind::Interrupted=>continue,_=>return Err(error()),
        }}
        Ok(v)
}
fn error()->io::Error{bad("not the exact sealed nonsecret offline operation")}
