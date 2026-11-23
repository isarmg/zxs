//! Checked sealed nonexecutable bytes for a fixed local worker request.
use super::bad;
use rustix::fs::{SealFlags,fcntl_get_seals};
use std::{fs::File,io::{self,Seek,Write},os::unix::fs::{FileExt,MetadataExt}};

pub(crate) fn sealed_owner(f:&File,max:usize,owner:(u32,u32))->io::Result<Vec<u8>>{
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

pub(crate) fn sealed(bytes:&[u8])->io::Result<File>{
    use rustix::fs::{MemfdFlags,fcntl_add_seals,memfd_create};
    let mut f=File::from(memfd_create(c"helper-fixed-origin",MemfdFlags::CLOEXEC|MemfdFlags::ALLOW_SEALING|MemfdFlags::NOEXEC_SEAL)?);
    f.write_all(bytes)?;let seals=SealFlags::WRITE|SealFlags::GROW|SealFlags::SHRINK|SealFlags::SEAL;
    fcntl_add_seals(&f,seals)?;
    if !fcntl_get_seals(&f)?.contains(seals|SealFlags::EXEC)||f.metadata()?.len()!=bytes.len() as u64{return Err(bad("incomplete fixed bootstrap artifact"));}
    f.rewind()?;Ok(f)
}
