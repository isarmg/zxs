//! An explicitly saved ext-family image, checked by e2fsck; this is not a
//! parser for directories, extents, bitmaps, backup superblocks or journals.
use super::leased;
use crate::pinned::invalid;
use std::{fmt,fs::File,io,os::unix::fs::FileExt};

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

pub(super) type Image=leased::Image<Extent>;
pub(super) fn read_frame(file:&File,length:u64)->io::Result<Extent>{
    let mut sb=[0;SUPER_LEN];file.read_exact_at(&mut sb,SUPER_OFFSET)?;frame(&sb,length)
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
pub(super) fn preen_summary(out:&[u8],extent:Extent)->bool{
    // e2fsprogs 1.47.2 prints its actual final counts even in preen.
    // Corrections may produce additional messages; never interpret or relay
    // them. Success still also needs a distinct real readonly five-pass check.
    out.strip_suffix(b"\n")
        .and_then(|v|v.rsplit(|&b|b==b'\n').next())
        .and_then(|line|line.strip_prefix(b"/image: "))
        .and_then(|row|counts(row,extent)).is_some()
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
