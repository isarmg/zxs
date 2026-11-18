//! Large FILE material uses a pinned regular inode, not a pathname reopened
//! for every chunk. A pin is obtained at the admitted cut; it is never a write permit.
use super::*;
use std::os::unix::fs::FileExt;

pub(crate) struct PinnedRegular{pub file:File,pub stamp:OriginalStamp,mount:u64}
impl PinnedRegular{
    pub(crate) fn unchanged(&self)->io::Result<()>{
        if record_stamp(&self.file,self.stamp.stat.size())?!=self.stamp.stat{return Err(invalid("immutable working material changed"));}
        Ok(())
    }
    pub(crate) fn named(&self,dir:&HeldDir,name:&CStr)->io::Result<()>{
        self.unchanged()?;let pin=dir.pin_record(name)?.ok_or_else(||invalid("original working material is no longer named"))?;
        if mounts::id(&pin)?!=self.mount||record_stamp(&pin,self.stamp.stat.size())?!=self.stamp.stat {
            return Err(invalid("working filename no longer identifies its original material"));
        }
        dir.require_named()
    }
}
impl HeldDir{
    pub(crate) fn pin_stream(&self,name:&CStr,exact:u64)->io::Result<Option<PinnedRegular>>{
        let Some(pin)=self.pin_record(name)? else{return Ok(None)};
        let stat=record_stamp(&pin,exact)?;let m=mounts::id(&self.file)?;
        if stat.size()!=exact||stat.dev!=self.entity.device||mounts::id(&pin)?!=m{return Err(invalid("working body length, mount or role differs"));}
        // SAFETY: verified private, regular, same-mount O_PATH original, no device or symlink opens.
        let fd=unsafe{zxs_open_pinned_record(pin.as_raw_fd(),0)};if fd<0{return Err(io::Error::last_os_error());}
        let file=unsafe{File::from_raw_fd(fd)};
        if record_stamp(&file,exact)?!=stat||mounts::id(&file)?!=m{return Err(invalid("working body pin was replaced"));}
        let birth=Birth::read(&file)?;let result=PinnedRegular{file,stamp:OriginalStamp{stat,birth},mount:m};
        result.named(self,name)?;Ok(Some(result))
    }
    pub(crate) fn link_stream(&self,name:&CStr,file:&File,original:&OriginalStamp,bytes:u64)->io::Result<()>{
        self.require_named()?;let m=file.metadata()?;
        if !m.is_file()||m.nlink()!=0||m.len()!=bytes||m.uid()!=local_uid()||m.mode()&0o7777!=0o600
            ||m.dev()!=self.entity.device||mounts::id(file)?!=mounts::id(&self.file)?||m.ino()!=original.stat.ino||m.dev()!=original.stat.dev{
            return Err(invalid("not the original anonymous working file at its target"));
        }
        original.birth.require(&Birth::read(file)?)?;file.sync_all()?;
        // SAFETY: creating exactly one name for our own anonymous original inode; no replacement.
        os_result(unsafe{zxs_link_anonymous(file.as_raw_fd(),self.file.as_raw_fd(),name.as_ptr())})?;self.file.sync_all()
    }
}
pub(crate) fn anonymous_stamp(file:&File)->io::Result<OriginalStamp>{
    let m=file.metadata()?;
    if !m.is_file()||m.nlink()!=0||m.ino()==0||m.uid()!=local_uid()||m.mode()&0o7777!=0o600{return Err(invalid("not the original private unlinked working file"));}
    Ok(OriginalStamp{stat:FileStamp{dev:m.dev(),ino:m.ino(),len:m.len(),ctime:(m.ctime(),m.ctime_nsec()),mtime:(m.mtime(),m.mtime_nsec())},
        birth:Birth::read(file)?})
}
unsafe extern "C"{fn zxs_allocate_stream(fd:c_int,total:u64)->c_int;}
pub(crate) fn allocate_stream(file:&File,bytes:u64)->io::Result<()>{
    // SAFETY: the caller holds the original unnamed file on the destination;
    // byte count already passed signed-offset and overflow checks.
    os_result(unsafe{zxs_allocate_stream(file.as_raw_fd(),bytes)})
}
pub(crate) fn read_at(file:&File,mut dst:&mut[u8],mut at:u64)->io::Result<()>{
    while !dst.is_empty(){
        match file.read_at(dst,at){Ok(0)=>return Err(io::Error::new(io::ErrorKind::UnexpectedEof,"short durable working material")),
            Ok(n)=>{dst=&mut dst[n..];at+=n as u64;},Err(e) if e.kind()==io::ErrorKind::Interrupted=>{},Err(e)=>return Err(e)}
    }Ok(())
}
pub(crate) fn write_at(file:&File,mut src:&[u8],mut at:u64)->io::Result<()>{
    while !src.is_empty(){
        match file.write_at(src,at){Ok(0)=>return Err(io::Error::new(io::ErrorKind::WriteZero,"short staged working material")),
            Ok(n)=>{src=&src[n..];at+=n as u64;},Err(e) if e.kind()==io::ErrorKind::Interrupted=>{},Err(e)=>return Err(e)}
    }Ok(())
}
