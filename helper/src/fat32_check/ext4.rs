//! An explicitly saved ext-family image, checked by e2fsck; this is not a
//! parser for directories, extents, bitmaps, backup superblocks or journals.
use crate::{inspect::{Input,Stamp},pinned::{Mount,invalid,local_data_filesystem}};
use helper_linux_evidence::diagnostic_process::{acquire_copy_read_lease,require_copy_read_lease};
use rustix::fs::{OFlags,fcntl_getfl};
use sha2::{Digest,Sha256};
use std::{fmt,fs::File,io,os::unix::fs::FileExt,path::Path};

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
        if self.source.copy_mount()?!=self.mount{return Err(invalid("selected ext copy mount changed"));}
        Ok(())
    }
    pub(super) fn reader(&self)->io::Result<File>{
        self.require()?;
        let file=self.source.reader()?;
        if Mount::on(&file)?!=Some(self.mount){return Err(invalid("selected ext copy reader mount changed"));}
        self.require()?;
        Ok(file)
    }
}

pub(super) struct Image{named:Option<Selected>,file:File,stamp:Stamp,mount:Mount,extent:Extent}
impl Image{
    pub(super) fn capture(path:&Path,expected:&[u8;32])->io::Result<Self>{
        let named=Selected::select(path)?;
        let file=named.reader()?;
        Self::verify(file,expected,Some(named))
    }
    // Only the dedicated worker calls this on the parent's selected readonly
    // stdin. The parent keeps that exact operand's name/ancestry evidence and
    // is responsible for validating it again after the worker is really reaped.
    pub(super) fn from_panel_reader(file:File,expected:&[u8;32])->io::Result<Self>{
        Self::verify(file,expected,None)
    }
    fn verify(file:File,expected:&[u8;32],named:Option<Selected>)->io::Result<Self>{
        // Never acquire a lease or probe a header on an arbitrary inherited
        // endpoint. No memfd or scratch copy, and no reopening its pathname.
        let flags=fcntl_getfl(&file)?;
        if flags.contains(OFlags::PATH)||flags&OFlags::ACCMODE!=OFlags::RDONLY
            ||!flags.contains(OFlags::NOATIME|OFlags::NONBLOCK){
            return Err(invalid("expected the original readonly ext copy reader"));
        }
        let stamp=Stamp::of(file.metadata()?)?;
        if !(2048..=i64::MAX as u64).contains(&stamp.len()){
            return Err(invalid("invalid ext copy length"));
        }
        local_data_filesystem(&file)?;
        let mount=Mount::on(&file)?.ok_or_else(||invalid("selected ext copy mount unavailable"))?;
        acquire_copy_read_lease(&file)?;
        if Stamp::of(file.metadata()?)?!=stamp || Mount::on(&file)?!=Some(mount){
            return Err(invalid("selected ext copy changed before acquiring its lease"));
        }
        if let Some(ref selected)=named{selected.require()?;}
        let mut sb=[0;SUPER_LEN];
        file.read_exact_at(&mut sb,SUPER_OFFSET)?;
        let extent=frame(&sb,stamp.len())?;
        if digest(&file,extent.bytes)?!=*expected{return Err(invalid("saved copy does not match the supplied SHA-256"));}
        let original=Self{named,file,stamp,mount,extent};
        original.require()?;
        Ok(original)
    }
    pub(super) fn file(&self)->&File{&self.file}
    pub(super) fn extent(&self)->Extent{self.extent}
    pub(super) fn require(&self)->io::Result<()>{
        require_copy_read_lease(&self.file)?;
        if let Some(ref selected)=self.named{selected.require()?;}
        let mut byte=[0];
        if Stamp::of(self.file.metadata()?)?!=self.stamp||Mount::on(&self.file)?!=Some(self.mount)
            ||self.file.read_at(&mut byte,self.extent.bytes)?!=0{
            return Err(invalid("selected ext copy name, mount or length changed"));
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

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum ResultKind{NoFindings,Uncorrected,Incomplete}
pub(crate) struct Report{result:ResultKind,upstream:i32,bytes:u64,filesystem_bytes:u64}
impl Report{
    pub(crate) fn exit(&self)->u8{
        match self.result{ResultKind::NoFindings=>0,ResultKind::Uncorrected=>1,ResultKind::Incomplete=>2}
    }

    // Private worker pipe, not an HTTP parameter format. The parent already
    // knows the terminal-selected digest and length and requires actual EOF.
    pub(super) const WIRE_SIZE:usize=57;
    pub(super) fn packet(&self,digest:&[u8;32])->[u8;Self::WIRE_SIZE]{
        let mut p=[0;Self::WIRE_SIZE];p[..4].copy_from_slice(b"XCR1");p[4]=self.exit();
        p[5..9].copy_from_slice(&self.upstream.to_be_bytes());
        p[9..17].copy_from_slice(&self.bytes.to_be_bytes());
        p[17..25].copy_from_slice(&self.filesystem_bytes.to_be_bytes());
        p[25..57].copy_from_slice(digest);p
    }
    pub(super) fn from_packet(p:&[u8],exit:Option<i32>,bytes:u64,digest:&[u8;32])->io::Result<Self>{
        let bad=||invalid("no matching complete receipt from the selected ext worker");
        if p.len()!=Self::WIRE_SIZE||&p[..4]!=b"XCR1"||p[9..17]!=bytes.to_be_bytes()||&p[25..57]!=digest{
            return Err(bad());
        }
        let upstream=i32::from_be_bytes(p[5..9].try_into().expect("four bytes"));
        let filesystem_bytes=u64::from_be_bytes(p[17..25].try_into().expect("eight bytes"));
        if !(2048..=bytes).contains(&filesystem_bytes)||!(0..=255).contains(&upstream){return Err(bad());}
        let result=match(p[4],upstream,exit){
            (0,0,Some(0))=>ResultKind::NoFindings,
            (1,4,Some(1))=>ResultKind::Uncorrected,
            (2,_,Some(2))=>ResultKind::Incomplete,
            _=>return Err(bad()),
        };
        Ok(Self{result,upstream,bytes,filesystem_bytes})
    }
    pub(super) fn panel_text(&self)->String{
        let status=match self.result{
            ResultKind::NoFindings=>"e2fsck 本次强制只读检查未报告问题；不代表原盘健康、备用超级块或文件正文已验证",
            ResultKind::Uncorrected=>"e2fsck 报告有未修复的问题；不能确认所有检查阶段都已完成",
            ResultKind::Incomplete=>"未能确认完整检查；不能判为正常",
        };
        format!("ext 文件系统：{status}。工具退出码：{}；镜像 / 文件系统范围（字节）：{} / {}。仅检查保存副本，未修复。",
            self.upstream,self.bytes,self.filesystem_bytes)
    }
}
impl fmt::Display for Report{
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{
        let result=match self.result{
            ResultKind::NoFindings=>"no-findings-reported-in-forced-readonly-check",
            ResultKind::Uncorrected=>"errors-left-uncorrected-full-coverage-not-confirmed",
            ResultKind::Incomplete=>"check-not-completed-or-not-confirmed",
        };
        write!(f,"format=ext-family checker=e2fsck-1.47.2 check=-fn operand=leased-readonly-saved-image repair=none result={result} checker_status={} image_bytes={} filesystem_bytes={}",self.upstream,self.bytes,self.filesystem_bytes)
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
    Report{result,upstream:code,bytes:extent.bytes,filesystem_bytes:extent.filesystem_bytes}
}

#[cfg(test)]
#[path="../../tests/fs_tools/ext4.rs"]
mod tests;
