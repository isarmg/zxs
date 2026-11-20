//! An explicitly saved ext-family image, checked by e2fsck; this is not a
//! parser for directories, extents, bitmaps, backup superblocks or journals.
use crate::{inspect::Input,pinned::{Mount,invalid}};
use helper_linux_evidence::diagnostic_process::{acquire_copy_read_lease,require_copy_read_lease};
use sha2::{Digest,Sha256};
use std::{fmt,fs::File,io::{self,Read},os::unix::fs::FileExt,path::Path};

const SUPER_OFFSET:u64=1024;
const SUPER_LEN:usize=1024;
const BANNER:&[u8]=b"e2fsck 1.47.2 (1-Jan-2025)\n";
const PASSES:[&[u8];5]=[
    b"Pass 1: Checking inodes, blocks, and sizes",
    b"Pass 2: Checking directory structure",
    b"Pass 3: Checking directory connectivity",
    b"Pass 4: Checking reference counts",
    b"Pass 5: Checking group summary information",
];

#[derive(Clone,Copy)]
pub(super) struct Extent {bytes:u64,filesystem_bytes:u64,blocks:u64,inodes:u32}
fn frame(sb:&[u8;SUPER_LEN],bytes:u64)->io::Result<Extent>{
    // Source: e2fsprogs 1.47.2 lib/ext2fs/ext2_fs.h. Read only the primary
    // envelope and features needed to know when e2fsck -n has no full scope.
    let u32le=|at:usize|u32::from_le_bytes(sb[at..at+4].try_into().expect("superblock field"));
    if sb[0x38..0x3a]!=[0x53,0xef]{return Err(invalid("expected an ext-family volume at offset zero, not a disk or container image"));}
    let (compat,incompat,ro)=(u32le(0x5c),u32le(0x60),u32le(0x64));
    let journal_inode=u32le(0xe0);
    if incompat&0x8!=0||sb[0xd0..0xe0]!=[0;16]||u32le(0xe4)!=0
        || (compat&0x4!=0)!=(journal_inode!=0){
        return Err(invalid("check not completed: external or inconsistent ext journal; no journal lookup or replay"));
    }
    if incompat&0x4!=0||ro&0x1_0000!=0||u32le(0xe8)!=0{
        return Err(invalid("check not completed: ext journal or orphan recovery is pending; nothing replayed"));
    }
    let log=u32le(0x18);
    if log>6{return Err(invalid("unsupported ext block size"));}
    let high=if incompat&0x80!=0{u64::from(u32le(0x150))}else{0};
    let blocks=high<<32|u64::from(u32le(0x04));
    let inodes=u32le(0);
    let filesystem_bytes=blocks.checked_mul(1024<<log).filter(|&n|n>=2048&&n<=bytes)
        .ok_or_else(||invalid("complete ext filesystem extent is not present in the selected copy"))?;
    if inodes==0{return Err(invalid("missing ext inode count"));}
    Ok(Extent{bytes,filesystem_bytes,blocks,inodes})
}

pub(super) struct Image{source:Input,file:File,mount:Mount,extent:Extent}
impl Image{
    pub(super) fn capture(path:&Path,expected:&[u8;32])->io::Result<Self>{
        // No memfd or scratch copy. The operator has supplied the saved copy;
        // preserve it in place and never acquire or accept a write descriptor.
        let source=Input::select_in(path,2048..=i64::MAX as u64)?;
        let mount=source.copy_mount()?;
        let mut file=source.reader()?;
        if Mount::on(&file)?!=Some(mount){return Err(invalid("selected ext copy mount changed"));}
        acquire_copy_read_lease(&file)?;
        source.recheck(&file)?;
        let mut sb=[0;SUPER_LEN];
        file.read_exact_at(&mut sb,SUPER_OFFSET)?;
        let extent=frame(&sb,source.len())?;
        if digest(&mut file,extent.bytes)?!=*expected{return Err(invalid("saved copy does not match the supplied SHA-256"));}
        let original=Self{source,file,mount,extent};
        original.require()?;
        Ok(original)
    }
    pub(super) fn file(&self)->&File{&self.file}
    pub(super) fn extent(&self)->Extent{self.extent}
    pub(super) fn require(&self)->io::Result<()>{
        require_copy_read_lease(&self.file)?;
        self.source.recheck(&self.file)?;
        let mut byte=[0];
        if self.source.copy_mount()?!=self.mount||Mount::on(&self.file)?!=Some(self.mount)
            ||self.file.read_at(&mut byte,self.extent.bytes)?!=0{
            return Err(invalid("selected ext copy name, mount or length changed"));
        }
        require_copy_read_lease(&self.file)
    }
}
fn digest(file:&mut File,length:u64)->io::Result<[u8;32]>{
    let mut hash=Sha256::new();let mut buf=[0;128*1024];let mut remaining=length;
    while remaining!=0{
        require_copy_read_lease(file)?;
        let limit=remaining.min(buf.len() as u64) as usize;
        let n=match file.read(&mut buf[..limit]){
            Ok(0)=>return Err(invalid("saved copy ended before the expected length")),
            Ok(n)=>n,Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,Err(e)=>return Err(e),
        };
        hash.update(&buf[..n]);remaining-=n as u64;
    }
    // A positional read of the actual endpoint in Image::require also checks EOF.
    Ok(hash.finalize().into())
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum ResultKind{NoFindings,Uncorrected,Incomplete}
pub(crate) struct Report{result:ResultKind,upstream:i32,extent:Extent}
impl Report{
    pub(crate) fn exit(&self)->u8{
        match self.result{ResultKind::NoFindings=>0,ResultKind::Uncorrected=>1,ResultKind::Incomplete=>2}
    }
}
impl fmt::Display for Report{
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{
        let result=match self.result{
            ResultKind::NoFindings=>"no-findings-reported-in-forced-readonly-check",
            ResultKind::Uncorrected=>"errors-left-uncorrected-full-coverage-not-confirmed",
            ResultKind::Incomplete=>"check-not-completed-or-not-confirmed",
        };
        write!(f,"format=ext-family checker=e2fsck-1.47.2 check=-fn operand=leased-readonly-saved-image repair=none result={result} checker_status={} image_bytes={} filesystem_bytes={}",self.upstream,self.extent.bytes,self.extent.filesystem_bytes)
    }
}

// -N /image is fixed for this exact tool: an on-image label cannot control
// the diagnostic grammar. Never pass through messages or on-image names.
fn complete(out:&[u8],extent:Extent)->bool{
    let Some(out)=out.strip_suffix(b"\n")else{return false;};
    let mut rows=out.split(|&x|x==b'\n');
    if PASSES.iter().any(|&pass|rows.next()!=Some(pass)){return false;}
    let Some(row)=rows.next().and_then(|r|r.strip_prefix(b"/image: "))else{return false;};
    rows.next().is_none() && counts(row,extent).is_some()
}
fn counts(row:&[u8],extent:Extent)->Option<()>{
    let s=std::str::from_utf8(row).ok()?;
    let (i,rest)=s.split_once(" files (")?;
    let (p,b)=rest.split_once("% non-contiguous), ")?;
    let (percent,fraction)=p.split_once('.')?;
    let (used_i,total_i)=i.split_once('/')?;
    let (used_b,total_b)=b.strip_suffix(" blocks")?.split_once('/')?;
    let dec=|s:&str|(!s.is_empty()&&s.bytes().all(|b|b.is_ascii_digit())).then(||s.parse::<u64>().ok()).flatten();
    let (ui,ti,ub,tb)=(dec(used_i)?,dec(total_i)?,dec(used_b)?,dec(total_b)?);
    (ti==u64::from(extent.inodes)&&ui<=ti&&tb==extent.blocks&&ub<=tb
        &&dec(percent)?<=100&&fraction.len()==1&&dec(fraction).is_some()).then_some(())
}
pub(super) fn classify(code:i32,out:&[u8],err:&[u8],extent:Extent)->Report{
    // 1 or 2 suggests changes/reboot, 8/16/32/128 are tool, usage,
    // cancellation or library failures. A combination is not solely bit 4.
    let result=match code{
        0 if err==BANNER&&complete(out,extent)=>ResultKind::NoFindings,
        4 if err.starts_with(BANNER)=>ResultKind::Uncorrected,
        _=>ResultKind::Incomplete,
    };
    Report{result,upstream:code,extent}
}

#[cfg(test)]
#[path="../../tests/fs_tools/ext4.rs"]
mod tests;
