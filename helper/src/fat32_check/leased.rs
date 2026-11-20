//! Hold exactly one selected saved volume, not a filesystem parser.
use crate::{inspect::{Input,Stamp},pinned::{Mount,invalid,local_data_filesystem}};
use helper_linux_evidence::diagnostic_process::{acquire_copy_read_lease,require_copy_read_lease};
use rustix::fs::{OFlags,fcntl_getfl};
use sha2::{Digest,Sha256};
use std::{fs::File,io,os::unix::fs::FileExt,path::Path};

// Kept by the terminal/HTTP parent, not serialized or selected by a child.
// Only O_PATH and initial metadata: no ordinary open, header read or lease.
pub(super) struct Selected{source:Input,mount:Mount}
impl Selected{
    pub(super) fn select(path:&Path)->io::Result<Self>{
        let source=Input::select_in(path,2048..=i64::MAX as u64)?;
        let mount=source.copy_mount()?;
        Ok(Self{source,mount})
    }
    pub(super) fn len(&self)->u64{self.source.len()}
    pub(super) fn require(&self)->io::Result<()>{
        if self.source.copy_mount()?!=self.mount{return Err(invalid("selected volume copy mount changed"));}
        Ok(())
    }
    pub(super) fn reader(&self)->io::Result<File>{
        self.require()?;
        let file=self.source.reader()?;
        if Mount::on(&file)?!=Some(self.mount){return Err(invalid("selected volume copy reader mount changed"));}
        self.require()?;
        Ok(file)
    }
}

pub(super) struct Image<F>{named:Option<Selected>,file:File,stamp:Stamp,mount:Mount,extent:F}
impl<F:Copy> Image<F>{
    pub(super) fn capture(path:&Path,expected:&[u8;32],frame:impl FnOnce(&File,u64)->io::Result<F>)->io::Result<Self>{
        let named=Selected::select(path)?;
        let file=named.reader()?;
        Self::verify(file,expected,Some(named),frame)
    }
    // Only the dedicated worker calls this on the parent's selected readonly
    // stdin. The parent keeps that exact operand's name/ancestry evidence and
    // is responsible for validating it again after the worker is really reaped.
    pub(super) fn from_panel_reader(file:File,expected:&[u8;32],frame:impl FnOnce(&File,u64)->io::Result<F>)->io::Result<Self>{
        Self::verify(file,expected,None,frame)
    }
    fn verify(file:File,expected:&[u8;32],named:Option<Selected>,frame:impl FnOnce(&File,u64)->io::Result<F>)->io::Result<Self>{
        // Never acquire a lease or probe a header on an arbitrary inherited
        // endpoint. No memfd or scratch copy, and no reopening its pathname.
        let flags=fcntl_getfl(&file)?;
        if flags.contains(OFlags::PATH)||flags&OFlags::ACCMODE!=OFlags::RDONLY
            ||!flags.contains(OFlags::NOATIME|OFlags::NONBLOCK){
            return Err(invalid("expected the original readonly volume copy reader"));
        }
        let stamp=Stamp::of(file.metadata()?)?;
        if !(2048..=i64::MAX as u64).contains(&stamp.len()){
            return Err(invalid("invalid volume copy length"));
        }
        local_data_filesystem(&file)?;
        let mount=Mount::on(&file)?.ok_or_else(||invalid("selected volume copy mount unavailable"))?;
        acquire_copy_read_lease(&file)?;
        if Stamp::of(file.metadata()?)?!=stamp || Mount::on(&file)?!=Some(mount){
            return Err(invalid("selected volume copy changed before acquiring its lease"));
        }
        if let Some(ref selected)=named{selected.require()?;}
        let extent=frame(&file,stamp.len())?;
        if digest(&file,stamp.len())?!=*expected{return Err(invalid("saved copy does not match the supplied SHA-256"));}
        let original=Self{named,file,stamp,mount,extent};
        original.require()?;
        Ok(original)
    }
    pub(super) fn file(&self)->&File{&self.file}
    pub(super) fn extent(&self)->F{self.extent}
    pub(super) fn require(&self)->io::Result<()>{
        require_copy_read_lease(&self.file)?;
        if let Some(ref selected)=self.named{selected.require()?;}
        let mut byte=[0];
        if Stamp::of(self.file.metadata()?)?!=self.stamp||Mount::on(&self.file)?!=Some(self.mount)
            ||self.file.read_at(&mut byte,self.stamp.len())?!=0{
            return Err(invalid("selected volume copy name, mount or length changed"));
        }
        require_copy_read_lease(&self.file)
    }
}
fn digest(file:&File,length:u64)->io::Result<[u8;32]>{
    let mut hash=Sha256::new();let mut buf=[0;128*1024];let mut at=0;
    while at!=length{
        require_copy_read_lease(file)?;
        let limit=(length-at).min(buf.len() as u64) as usize;
        let n=match file.read_at(&mut buf[..limit],at){
            Ok(0)=>return Err(invalid("saved copy ended before the expected length")),
            Ok(n)=>n,Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,Err(e)=>return Err(e),
        };
        hash.update(&buf[..n]);at+=n as u64;
    }
    // A positional read of the actual endpoint in Image::require also checks EOF.
    Ok(hash.finalize().into())
}
