//! Offline XFS image admission and classification of actual `xfs_repair` output.
//! The native tool, not this envelope reader, checks the filesystem structures.
use super::leased;
use crate::pinned::invalid;
use std::{fmt,fs::File,io,iter::Peekable,os::unix::fs::FileExt};

const HEADER:usize=512;
#[derive(Clone,Copy)]
pub(super) struct Extent{bytes:u64,filesystem_bytes:u64,ags:u32}
pub(super) type Image=leased::Image<Extent>;

pub(super) fn read_frame(file:&File,length:u64)->io::Result<Extent>{
    let mut sb=[0;HEADER];file.read_exact_at(&mut sb,0)?;frame(&sb,length)
}
fn frame(sb:&[u8;HEADER],bytes:u64)->io::Result<Extent>{
    // xfsprogs 6.13.0 libxfs/xfs_format.h: struct xfs_dsb. It is big-endian
    // except for CRC (which is deliberately left to the upstream checker).
    let u16be=|i:usize|u16::from_be_bytes(sb[i..i+2].try_into().expect("superblock field"));
    let u32be=|i:usize|u32::from_be_bytes(sb[i..i+4].try_into().expect("superblock field"));
    let u64be=|i:usize|u64::from_be_bytes(sb[i..i+8].try_into().expect("superblock field"));
    if &sb[..4]!=b"XFSB"{return Err(invalid("expected a single XFS volume beginning at offset zero"));}
    if u64be(16)!=0 || u64be(24)!=0 || u32be(92)!=0 {
        return Err(invalid("XFS realtime storage is not available in this single-image check"));
    }
    let log=u64be(48);let logblocks=u32be(96);
    if log==0||logblocks==0 {return Err(invalid("XFS external or missing log is not supported"));}
    if sb[126]!=0 || (u16be(100)&0xf==5 && u32be(216)&0x10!=0){
        return Err(invalid("XFS setup is incomplete or the superblock marks the volume as needing repair"));
    }
    let block=u32be(4);let sectors=u32::from(u16be(102));let blocks=u64be(8);
    let agblocks=u64::from(u32be(84));let ags=u32be(88);
    let agshift=u32::from(sb[124]);
    if !(512..=65536).contains(&block)||!block.is_power_of_two()||block.trailing_zeros()!=u32::from(sb[120])
        ||!(512..=block.min(32768)).contains(&sectors)||!sectors.is_power_of_two()||sectors.trailing_zeros()!=u32::from(sb[121])
        ||!(1..=i32::MAX as u32).contains(&ags)||agblocks==0
        ||u64::BITS-(agblocks-1).leading_zeros()!=agshift||blocks.div_ceil(agblocks)!=u64::from(ags){
        return Err(invalid("inconsistent XFS image envelope"));
    }
    let filesystem_bytes=blocks.checked_mul(u64::from(block)).filter(|&n|(2048..=bytes).contains(&n))
        .ok_or_else(||invalid("complete XFS filesystem extent is not present in the saved copy"))?;
    // Unlike a linear block count, XFS's fsblock can include gaps from
    // rounding each allocation group's bit-field up to its power of two.
    let log_ag=log>>agshift;
    let log_local=log&((1u64<<agshift)-1);
    if log_ag>=u64::from(ags) || log_local+u64::from(logblocks)>agblocks.min(blocks-log_ag*agblocks){
        return Err(invalid("XFS internal log is outside the declared allocation group"));
    }
    Ok(Extent{bytes,filesystem_bytes,ags})
}

// Exact C-locale grammar of the fixed 6.13 binary with -n -f -P -t 0.
// All other diagnostics, including a dirty or unreadable log which -n can
// continue past, prohibit the positive result even if the tool exits 0.
const BOUNDARY:[&[u8];3]=[
    b"Cannot get host filesystem geometry.",
    b"Repair may fail if there is a sector size mismatch between",
    b"the image and the host filesystem.",
];
const PHASE1:&[u8]=b"Phase 1 - find and verify superblock...";
const BEFORE_AG3:[&[u8];7]=[
    b"Phase 2 - using internal log",
    b"        - zero log...",
    b"        - scan filesystem freespace and inode maps...",
    b"        - found root inode chunk",
    b"Phase 3 - for each AG...",
    b"        - scan (but don't clear) agi unlinked lists...",
    b"        - process known inodes and perform inode discovery...",
];
const BEFORE_AG4:[&[u8];4]=[
    b"        - process newly discovered inodes...",
    b"Phase 4 - check for duplicate blocks...",
    b"        - setting up duplicate extent list...",
    b"        - check for inodes claiming duplicate blocks...",
];
const END:[&[u8];6]=[
    b"Phase 6 - check inode connectivity...",
    b"        - traversing filesystem ...",
    b"        - traversal finished ...",
    b"        - moving disconnected inodes to lost+found ...",
    b"Phase 7 - verify link counts...",
    b"No modify flag set, skipping filesystem flush and exiting.",
];
const SKIP5:&[u8]=b"No modify flag set, skipping phase 5\n";
const AG:&[u8]=b"        - agno = ";
fn rows_match<'a>(rows:&mut impl Iterator<Item=&'a [u8]>,want:&[&[u8]])->bool{
    want.iter().all(|&row|rows.next()==Some(row))
}
fn geometry_notice<'a,I:Iterator<Item=&'a [u8]>>(rows:&mut Peekable<I>)->bool{
    rows.peek().is_none_or(|&line|line!=BOUNDARY[0])||rows_match(rows,&BOUNDARY)
}
fn groups<'a,I:Iterator<Item=&'a [u8]>>(rows:&mut Peekable<I>,count:u32)->bool{
    // Allocation scales only with the already bounded collected output, not
    // with the untrusted u32 allocation group count in the image header.
    let mut ids=Vec::new();
    while let Some(raw)=rows.peek().and_then(|r|r.strip_prefix(AG)){
        if raw.is_empty()||raw.len()>10||(raw.len()>1&&raw[0]==b'0')||!raw.iter().all(u8::is_ascii_digit){return false;}
        let Some(id)=std::str::from_utf8(raw).ok().and_then(|s|s.parse::<u32>().ok())else{return false;};
        ids.push(id);rows.next();
    }
    if ids.len()!=count as usize{return false;}
    ids.sort_unstable();ids.into_iter().eq(0..count)
}
fn complete(out:&[u8],err:&[u8],extent:Extent)->bool{
    if out!=SKIP5{return false;}
    let Some(err)=err.strip_suffix(b"\n")else{return false;};
    let mut r=err.split(|&c|c==b'\n').peekable();
    geometry_notice(&mut r)&&r.next()==Some(PHASE1)&&geometry_notice(&mut r)
        &&rows_match(&mut r,&BEFORE_AG3)&&groups(&mut r,extent.ags)
        &&rows_match(&mut r,&BEFORE_AG4)&&groups(&mut r,extent.ags)
        &&rows_match(&mut r,&END)&&r.next().is_none()
}

pub(crate) struct Report {no_findings:bool,upstream:i32,bytes:u64,filesystem_bytes:u64}
impl Report{
    pub(crate) fn exit(&self)->u8{if self.no_findings{0}else{2}}
    pub(super) const WIRE_SIZE:usize=57;
    pub(super) fn packet(&self,digest:&[u8;32])->[u8;Self::WIRE_SIZE]{
        let mut p=[0;Self::WIRE_SIZE];p[..4].copy_from_slice(b"XFR1");p[4]=self.exit();
        p[5..9].copy_from_slice(&self.upstream.to_be_bytes());
        p[9..17].copy_from_slice(&self.bytes.to_be_bytes());
        p[17..25].copy_from_slice(&self.filesystem_bytes.to_be_bytes());
        p[25..].copy_from_slice(digest);p
    }
    pub(super) fn from_packet(p:&[u8],exit:Option<i32>,bytes:u64,digest:&[u8;32])->io::Result<Self>{
        let bad=||invalid("no matching complete receipt from the selected XFS worker");
        if p.len()!=Self::WIRE_SIZE||&p[..4]!=b"XFR1"||p[9..17]!=bytes.to_be_bytes()||&p[25..]!=digest{return Err(bad());}
        let upstream=i32::from_be_bytes(p[5..9].try_into().expect("four bytes"));
        let filesystem_bytes=u64::from_be_bytes(p[17..25].try_into().expect("eight bytes"));
        if !(0..=255).contains(&upstream)||!(2048..=bytes).contains(&filesystem_bytes){return Err(bad());}
        let no_findings=match(p[4],upstream,exit){(0,0,Some(0))=>true,(2,_,Some(2))=>false,_=>return Err(bad())};
        Ok(Self{no_findings,upstream,bytes,filesystem_bytes})
    }
    pub(super) fn panel_text(&self)->String{
        let status=if self.no_findings{"本次只读检查未报告问题；上游按设计跳过阶段 5，不代表原盘健康或文件正文已验证"}
            else{"未能确认此次检查；有诊断、日志待处理或工具未完成，不能判为正常"};
        format!("XFS：{status}。工具退出码：{}；镜像 / 文件系统范围（字节）：{} / {}。仅检查保存副本，未修复。",
            self.upstream,self.bytes,self.filesystem_bytes)
    }
}
impl fmt::Display for Report{
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{
        let result=if self.no_findings{"no-findings-reported-in-readonly-check"}else{"check-not-confirmed"};
        write!(f,"format=XFS checker=xfs_repair-6.13.0 check=-nf operand=leased-readonly-saved-image repair=none result={result} checker_status={} image_bytes={} filesystem_bytes={}",
               self.upstream,self.bytes,self.filesystem_bytes)
    }
}
pub(super) fn classify(code:i32,out:&[u8],err:&[u8],extent:Extent)->Report{
    Report{no_findings:code==0&&complete(out,err,extent),upstream:code,bytes:extent.bytes,filesystem_bytes:extent.filesystem_bytes}
}

#[cfg(test)]
#[path="../../tests/fs_tools/xfs.rs"]
mod tests;
