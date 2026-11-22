//! Admission of a single saved NTFS volume and limited ntfsfix observations.
//! This is an envelope check, not an alternative filesystem checker.
use super::leased;
use crate::pinned::invalid;
use std::{fmt,fs::File,io,os::unix::fs::FileExt};

const BOOT:usize=512;
// On a regular file the pinned libntfs device.c:599–615 doubles a signed
// upper bound, then adds low + high. Up to 2^62, both remain in range.
const MAX_PROBE:u64=1<<62;
#[derive(Clone,Copy)]
pub(super) struct Extent{bytes:u64,declared_bytes:u64}
pub(super) type Image=leased::Image<Extent>;

pub(super) fn read_frame(file:&File,bytes:u64)->io::Result<Extent>{
    let mut boot=[0;BOOT];file.read_exact_at(&mut boot,0)?;frame(&boot,bytes)
}
fn frame(boot:&[u8;BOOT],bytes:u64)->io::Result<Extent>{
    if &boot[3..11]!=b"NTFS    "{return Err(invalid("expected a single NTFS volume starting at offset zero"));}
    let sector=u64::from(u16::from_le_bytes(boot[11..13].try_into().expect("sector size")));
    if bytes>MAX_PROBE{return Err(invalid("NTFS image exceeds the pinned tool's safe size probe"));}
    if !matches!(sector,512|1024|2048|4096)||!bytes.is_multiple_of(sector){
        return Err(invalid("expected a whole number of supported NTFS sectors"));
    }
    // The on-disk BPB count is signed. The pinned ntfsfix checks the
    // *actual last* whole sector as the backup only when that sector is
    // beyond the declared count (ntfsfix.c:1361–1427).
    let sectors=i64::from_le_bytes(boot[40..48].try_into().expect("NTFS sector count"));
    let declared_bytes=u64::try_from(sectors).ok().filter(|&n|n!=0)
        .and_then(|n|n.checked_mul(sector))
        .filter(|&n|bytes.checked_sub(n).is_some_and(|extra|extra>=sector))
        .ok_or_else(||invalid("saved NTFS copy does not contain the declared extent and an alternate boot sector"))?;
    Ok(Extent{bytes,declared_bytes})
}

// Accept only the initial library RO open and exact normal end from
// Debian 1:2022.10.3-5+deb13u3. ntfsfix can try a different open after
// FAILED, so neither status zero nor its last line alone suffices.
const BEGIN:&[u8]=b"Mounting volume... OK\nProcessing of $MFT and $MFTMirr completed successfully.\nChecking the alternate boot sector... OK\nNTFS volume version is ";
const END:&[u8]=b".\nNTFS partition /image was processed successfully.\n";
fn native_version(s:&[u8])->bool{
    if matches!(s,b"1.1"|b"1.2"|b"3.0"|b"3.1"){return true;}
    s.strip_prefix(b"2.").is_some_and(|minor|{
        (1..=3).contains(&minor.len())&&(minor.len()==1||minor[0]!=b'0')
        &&minor.iter().all(u8::is_ascii_digit)
        &&std::str::from_utf8(minor).ok().and_then(|v|v.parse::<u8>().ok()).is_some()
    })
}
fn complete(out:&[u8],err:&[u8])->bool{
    err.is_empty()&&out.strip_prefix(BEGIN).and_then(|s|s.strip_suffix(END)).is_some_and(native_version)
}
pub(crate) struct Report {observed:bool,upstream:i32,bytes:u64,declared_bytes:u64}
impl Report {
    pub(crate) fn exit(&self)->u8{if self.observed{0}else{2}}
    pub(super) const WIRE_SIZE:usize=57;
    pub(super) fn packet(&self,digest:&[u8;32])->[u8;Self::WIRE_SIZE]{
        let mut p=[0;Self::WIRE_SIZE];p[..4].copy_from_slice(b"NFR1");p[4]=self.exit();
        p[5..9].copy_from_slice(&self.upstream.to_be_bytes());
        p[9..17].copy_from_slice(&self.bytes.to_be_bytes());
        p[17..25].copy_from_slice(&self.declared_bytes.to_be_bytes());
        p[25..].copy_from_slice(digest);p
    }
    pub(super) fn from_packet(p:&[u8],exit:Option<i32>,bytes:u64,digest:&[u8;32])->io::Result<Self>{
        let bad=||invalid("no matching complete receipt from the selected NTFS worker");
        if p.len()!=Self::WIRE_SIZE||&p[..4]!=b"NFR1"||p[9..17]!=bytes.to_be_bytes()||&p[25..]!=digest{return Err(bad());}
        let upstream=i32::from_be_bytes(p[5..9].try_into().expect("four bytes"));
        let declared_bytes=u64::from_be_bytes(p[17..25].try_into().expect("eight bytes"));
    if !(0..=255).contains(&upstream)||declared_bytes<512||!declared_bytes.is_multiple_of(512)||!bytes.is_multiple_of(512)||bytes>MAX_PROBE
            ||bytes.checked_sub(declared_bytes).is_none_or(|extra|extra<512){return Err(bad());}
        let observed=match(p[4],upstream,exit){(0,0,Some(0))=>true,(2,_,Some(2))=>false,_=>return Err(bad())};
        Ok(Self{observed,upstream,bytes,declared_bytes})
    }
    pub(super) fn panel_text(&self)->String{
        let state=if self.observed{"上游完成了有限的 MFT、MFT 镜像及备用引导扇区观察"}
            else{"未能确认有限观察；工具报错、有其他诊断或未完整结束"};
        format!("NTFS：{state}。工具退出码：{}；镜像 / 声明范围（字节）：{} / {}。未验证日志是否干净、Windows 休眠状态或普通文件正文，不是 chkdsk，未修复。",
                self.upstream,self.bytes,self.declared_bytes)
    }
}
impl fmt::Display for Report{
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{
        let result=if self.observed{"limited-metadata-observation-completed"}else{"limited-observation-not-confirmed"};
        write!(f,"format=NTFS checker=ntfsfix-2022.10.3-deb13u3 check=-n operand=leased-readonly-saved-image result={result} checker_status={} image_bytes={} declared_bytes={} journal=unverified hibernation=unverified scope=not-chkdsk repair=none",
               self.upstream,self.bytes,self.declared_bytes)
    }
}
pub(super) fn classify(code:i32,out:&[u8],err:&[u8],extent:Extent)->Report{
    Report{observed:code==0&&complete(out,err),upstream:code,bytes:extent.bytes,declared_bytes:extent.declared_bytes}
}

#[cfg(test)]
#[path="../../tests/fs_tools/ntfs.rs"]
mod tests;
