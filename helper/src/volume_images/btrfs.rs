//! Single-device raw Btrfs image admission; btrfs-progs does the checking.
use super::leased;
use crate::pinned::invalid;
use std::{fmt,fs::File,io,os::unix::fs::FileExt};

const PRIMARY:u64=64<<10;
const SUPER_BYTES:usize=4096;
const MINIMUM:u64=PRIMARY+SUPER_BYTES as u64;
const SEEDING:u64=1<<32;
const METADUMP:u64=1<<33;
const METADUMP_V2:u64=1<<34;
// These 6.14 super flags are persistent markers for incomplete offline
// operations, not a generic feature allowlist. In particular the first
// metadata-UUID change transaction leaves CHANGING_FSID_V2 set.
const CHANGING:u64=(1<<35)|(1<<36)|(1<<38)|(1<<39)|(1<<40);
const ZONED:u64=1<<12;
const FREE_SPACE_TREE:u64=1;

#[derive(Clone,Copy)]
pub(super) struct Extent{bytes:u64,device_bytes:u64,fsid:[u8;16],logged:bool,free_tree:bool}
pub(super) type Image=leased::Image<Extent>;
pub(super) fn read_frame(file:&File,bytes:u64)->io::Result<Extent>{
    let mut sb=[0;SUPER_BYTES];file.read_exact_at(&mut sb,PRIMARY)?;frame(&sb,bytes)
}
fn frame(sb:&[u8;SUPER_BYTES],bytes:u64)->io::Result<Extent>{
    // Exact packed btrfs_super_block and btrfs_dev_item in 6.14-1
    // kernel-shared/uapi/btrfs_tree.h. Leave the native checksum and all
    // trees, extents and mirror selection to the checker, not a Rust fsck.
    let u64le=|i:usize|u64::from_le_bytes(sb[i..i+8].try_into().expect("superblock field"));
    if &sb[64..72]!=b"_BHRfS_M"||u64le(48)!=PRIMARY{
        return Err(invalid("expected the primary Btrfs superblock of one raw volume image"));
    }
    let flags=u64le(56);
    if u64le(136)!=1 || flags&(SEEDING|METADUMP|METADUMP_V2)!=0{
        return Err(invalid("expected a self-contained single Btrfs device, not a device set, seed or metadata dump"));
    }
    if flags&CHANGING!=0{return Err(invalid("Btrfs records an unfinished UUID or format conversion"));}
    // On a regular file the native tool can only *emulate* zone geometry.
    if u64le(188)&ZONED!=0{return Err(invalid("zoned Btrfs geometry cannot be established from this ordinary saved image"));}
    let device_bytes=u64le(209); // dev_item starts at 201; total_bytes follows devid.
    if !(MINIMUM..=bytes).contains(&device_bytes)||u64le(112)!=device_bytes{
        return Err(invalid("the original declared single-device extent is not fully present or is inconsistent"));
    }
    Ok(Extent{bytes,device_bytes,fsid:sb[32..48].try_into().expect("UUID"),
        logged:u64le(96)!=0,free_tree:u64le(180)&FREE_SPACE_TREE!=0})
}

// C-locale normal output from the fixed btrfs check --readonly (no progress,
// repair, backup, alternate tree, or data-csum flags). Warnings about missing
// devices, bad mirrors or quota rescan can accompany a zero status. Do not
// report a positive outcome on any extra text or an incomplete phase list.
const OUT_HEAD:[&[u8];2]=[b"Opening filesystem to check...",b"Checking filesystem on /image"];
const COUNTS:[&[u8];7]=[
    b"total csum bytes: ",b"total tree bytes: ",b"total fs tree bytes: ",
    b"total extent tree bytes: ",b"btree space waste bytes: ",
    b"file data blocks allocated: ",b" referenced ",
];
fn uuid_wire(bytes:&[u8;16])->[u8;36]{
    const DIGITS:&[u8;16]=b"0123456789abcdef";
    let mut out=[b'-';36];let mut i=0;
    for (p,&v) in bytes.iter().enumerate(){
        if matches!(p,4|6|8|10){i+=1;}
        out[i]=DIGITS[usize::from(v>>4)];out[i+1]=DIGITS[usize::from(v&15)];i+=2;
    }
    out
}
fn decimal(bytes:&[u8])->bool{
    (1..=20).contains(&bytes.len())&&(bytes.len()==1||bytes[0]!=b'0')
        &&bytes.iter().all(u8::is_ascii_digit)
        &&std::str::from_utf8(bytes).ok().and_then(|raw|raw.parse::<u64>().ok()).is_some()
}
fn complete(out:&[u8],err:&[u8],extent:Extent)->bool{
    let (Some(out),Some(err))=(out.strip_suffix(b"\n"),err.strip_suffix(b"\n"))else{return false;};
    let mut rows=out.split(|&x|x==b'\n');
    if !OUT_HEAD.iter().all(|&line|rows.next()==Some(line)){return false;}
    let Some(id)=rows.next().and_then(|l|l.strip_prefix(b"UUID: "))else{return false;};
    if !id.eq_ignore_ascii_case(&uuid_wire(&extent.fsid)){return false;}
    let Some(used)=rows.next().and_then(|l|l.strip_prefix(b"found "))
        .and_then(|s|s.strip_suffix(b" bytes used, no error found"))else{return false;};
    if !decimal(used)||!COUNTS.iter().all(|&p|rows.next().and_then(|l|l.strip_prefix(p)).is_some_and(decimal))||rows.next().is_some(){
        return false;
    }
    let mut phases=err.split(|&x|x==b'\n');
    let expected:[&[u8];7]=[
        if extent.logged{b"[1/8] checking log"}else{b"[1/8] checking log skipped (none written)"},
        b"[2/8] checking root items",b"[3/8] checking extents",
        if extent.free_tree{b"[4/8] checking free space tree"}else{b"[4/8] checking free space cache"},
        b"[5/8] checking fs roots",b"[6/8] checking only csums items (without verifying data)",
        b"[7/8] checking root refs",
    ];
    expected.iter().all(|&line|phases.next()==Some(line))
        &&phases.next().is_some_and(|l|matches!(l,b"[8/8] checking quota groups"|b"[8/8] checking quota groups skipped (not enabled on this FS)"))
        &&phases.next().is_none()
}

pub(crate) struct Report{completed:bool,upstream:i32,bytes:u64,device_bytes:u64}
impl Report{
    pub(crate) fn exit(&self)->u8{if self.completed{0}else{2}}
    pub(super) const WIRE_SIZE:usize=57;
    pub(super) fn packet(&self,digest:&[u8;32])->[u8;Self::WIRE_SIZE]{
        let mut p=[0;Self::WIRE_SIZE];p[..4].copy_from_slice(b"BFR1");p[4]=self.exit();
        p[5..9].copy_from_slice(&self.upstream.to_be_bytes());
        p[9..17].copy_from_slice(&self.bytes.to_be_bytes());
        p[17..25].copy_from_slice(&self.device_bytes.to_be_bytes());
        p[25..].copy_from_slice(digest);p
    }
    pub(super) fn from_packet(p:&[u8],exit:Option<i32>,bytes:u64,digest:&[u8;32])->io::Result<Self>{
        let bad=||invalid("no matching complete receipt from the selected Btrfs worker");
        if p.len()!=Self::WIRE_SIZE||&p[..4]!=b"BFR1"||p[9..17]!=bytes.to_be_bytes()||&p[25..]!=digest{return Err(bad());}
        let upstream=i32::from_be_bytes(p[5..9].try_into().expect("four bytes"));
        let device_bytes=u64::from_be_bytes(p[17..25].try_into().expect("eight bytes"));
        if !(0..=255).contains(&upstream)||!(MINIMUM..=bytes).contains(&device_bytes){return Err(bad());}
        let completed=match(p[4],upstream,exit){(0,0,Some(0))=>true,(2,_,Some(2))=>false,_=>return Err(bad())};
        Ok(Self{completed,upstream,bytes,device_bytes})
    }
    pub(super) fn panel_text(&self)->String{
        let state=if self.completed{"本次单设备副本的原生结构检查未报告问题"}
            else{"未能确认结构检查完整结束；工具报错、额外诊断或有未完成步骤"};
        format!("Btrfs：{state}。工具退出码：{}；镜像 / 声明设备范围（字节）：{} / {}。未验证每份元数据镜像、正文或原盘健康；未重放日志、未修复。",
                self.upstream,self.bytes,self.device_bytes)
    }
}
impl fmt::Display for Report{
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{
        let result=if self.completed{"no-findings-reported-in-readonly-structure-check"}else{"structure-check-not-confirmed"};
        write!(f,"format=Btrfs checker=btrfs-progs-6.14 check=--readonly operand=leased-readonly-single-device-image result={result} checker_status={} image_bytes={} device_bytes={} data=unverified replay=none repair=none",
               self.upstream,self.bytes,self.device_bytes)
    }
}
pub(super) fn classify(code:i32,out:&[u8],err:&[u8],extent:Extent)->Report{
    Report{completed:code==0&&complete(out,err,extent),upstream:code,bytes:extent.bytes,device_bytes:extent.device_bytes}
}

#[cfg(test)]
#[path="../../tests/fs_tools/btrfs.rs"]
mod tests;
