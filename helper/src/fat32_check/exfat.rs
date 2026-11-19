//! Only the image/volume correspondence and the bounded public result. Real
//! allocation, directories, checksums and corruption checks belong to fsck.exfat.
mod receipt;
use crate::pinned::invalid;
use std::{fmt,io};

#[derive(Clone,Copy,Debug)]
pub(super) struct Framing{pub(super) length:u64,pub(super) sector:u16,flags:u16}
pub(super) fn framing(boot:&[u8;512],length:u64)->io::Result<Framing>{
    let bad=||io::Error::new(io::ErrorKind::Unsupported,"not a supported whole exFAT 1.00 single-FAT volume image");
    // An extracted volume can legitimately retain its original partition offset.
    // The selected operand still has to start at the boot sector and end at its
    // declared volume size. Reject unsupported framing, do not declare it damage.
    if boot[..3]!=[0xeb,0x76,0x90] || &boot[3..11]!=b"EXFAT   " || boot[11..64].iter().any(|&b|b!=0)
        || boot[510..]!=[0x55,0xaa] || boot[104..106]!=[0,1] || boot[110]!=1 || boot[106]&1!=0
        || !(9..=12).contains(&boot[108]) || u16::from(boot[108])+u16::from(boot[109])>25
        || length<1<<20 {return Err(bad());}
    let sector=1u16<<boot[108];
    let sectors=u64::from_le_bytes(boot[72..80].try_into().expect("eight bytes"));
    if sectors.checked_mul(u64::from(sector))!=Some(length){return Err(bad());}
    Ok(Framing{length,sector,flags:u16::from_le_bytes([boot[106],boot[107]])})
}

#[derive(Debug,PartialEq,Eq)]
enum Outcome{NoFindings,VolumeFlags,ExtraDiagnostics,Uncorrected,IncompleteUnresolved}
pub(crate) struct Report{state:Outcome,frame:Framing,counts:Option<(u32,u32)>}
impl Report{
    pub(crate) fn exit(&self)->u8{match self.state{
        Outcome::NoFindings=>0, Outcome::Uncorrected|Outcome::VolumeFlags=>1,
        Outcome::ExtraDiagnostics|Outcome::IncompleteUnresolved=>2,
    }}
}
impl fmt::Display for Report{
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{
        let (status,coverage,code)=match self.state{
            Outcome::NoFindings=>("no-findings-reported","checker-returned-clean",0),
            Outcome::VolumeFlags=>("input-volume-flags-need-attention","checker-returned-clean",0),
            Outcome::ExtraDiagnostics=>("additional-diagnostics-not-classified","not-confirmed",0),
            Outcome::Uncorrected=>("checker-reported-unresolved-errors","not-confirmed",4),
            Outcome::IncompleteUnresolved=>("checker-reported-unresolved-condition","incomplete-or-unknown",4),
        };
        write!(f,"format=exFAT checker=exfatprogs-1.2.9 check=-n operand=independent-sealed-image repair=none native_exit={code} result={status} coverage={coverage} image_bytes={} sector_bytes={} volume_dirty={} media_failure_flag={}",self.frame.length,self.frame.sector,self.frame.flags&2!=0,self.frame.flags&4!=0)?;
        if let Some((d,n))=self.counts{write!(f," reported_directories={d} reported_files={n}")?;}
        Ok(())
    }
}
fn integer(s:&[u8])->Option<u32>{
    if s.is_empty()||s.len()>10||(s.len()>1 && s[0]==b'0')||!s.iter().all(u8::is_ascii_digit){return None;}
    std::str::from_utf8(s).ok()?.parse().ok()
}
fn pair(line:&[u8],start:&[u8],between:&[u8])->Option<(u32,u32)>{
    let raw=line.strip_prefix(start)?;
    let i=raw.iter().position(|b|!b.is_ascii_digit())?;
    Some((integer(&raw[..i])?,integer(raw[i..].strip_prefix(between)?)?))
}
fn counts(line:&[u8],header:&[u8],frame:Framing)->Option<(u32,u32)>{
    let(d,f)=pair(line,header,b", files ")?;
    // Root is counted. These are only the tool's reported counts, not a
    // second directory walker. Even one directory entry takes at least 32 B.
    valid_counts(d,f,frame).then_some((d,f))
}
fn valid_counts(d:u32,f:u32,frame:Framing)->bool{
    d>=1 && u64::from(d)+u64::from(f)<=1+frame.length/32
}
pub(super) fn classify(code:i32,out:&[u8],err:&[u8],frame:Framing)->io::Result<Report>{
    const BANNER:&[u8]=b"exfatprogs version : 1.2.9";
    let mut lines=out.strip_suffix(b"\n").ok_or_else(||invalid("incomplete exFAT checker output"))?.split(|b|*b==b'\n');
    if lines.next()!=Some(BANNER){return Err(invalid("expected pinned exFAT checker banner not received"));}
    let incomplete=||invalid("exFAT check did not yield a consistent complete result");
    let (state,counts)=match code{
        0=>{
            let c=counts(lines.next_back().ok_or_else(incomplete)?,b"/image: clean. directories ",frame).ok_or_else(incomplete)?;
            let state=if lines.next().is_some()||!err.is_empty(){Outcome::ExtraDiagnostics}
                else if frame.flags&6!=0{Outcome::VolumeFlags}else{Outcome::NoFindings};
            (state,Some(c))
        }
        4=>{
            // Upstream also maps some early -EINVAL and I/O paths to 4. It may
            // even print "clean" on that path because the display ignores ret.
            // Only an explicit zero-fixes, positive-error final pair supports
            // a findings report, and even then traversal is not guaranteed.
            let tail=lines.next_back().and_then(|l|pair(l,b"/image: files corrupted ",b", files fixed "));
            let data=lines.next_back().and_then(|l|counts(l,b"/image: corrupted. directories ",frame));
            if matches!(tail,Some((n,0)) if n>0) && data.is_some(){(Outcome::Uncorrected,data)}
            else{(Outcome::IncompleteUnresolved,None)}
        }
        8=>return Err(invalid("exFAT checker reported an operational failure; inspection incomplete")),
        16=>return Err(invalid("exFAT checker rejected its invocation; inspection incomplete")),
        _=>return Err(invalid("unexpected exFAT checker status in no-repair mode; inspection incomplete")),
    };
    Ok(Report{state,frame,counts})
}

#[cfg(test)]
#[path="../../tests/fs_tools/exfat.rs"]
mod tests;
