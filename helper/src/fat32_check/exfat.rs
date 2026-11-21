//! Only the image/volume correspondence and the bounded public result. Real
//! allocation, directories, checksums and corruption checks belong to fsck.exfat.
mod receipt;
use crate::pinned::invalid;
use std::{fmt,fs::File,io,os::unix::fs::FileExt};

const BANNER:&[u8]=b"exfatprogs version : 1.2.9";

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

// Repair and its independent recheck may change volume flags, allocation and
// directory entries, but must not replace the volume's BPB identity/geometry.
// This is not a second filesystem checker or a backup boot-sector parser.
#[derive(Clone,Copy)]
pub(super) struct RepairFrame{pub(super) view:Framing,original_bpb:[u8;45]}
pub(super) type RepairImage=super::leased::Image<RepairFrame>;
pub(super) fn repair_frame(file:&File,length:u64)->io::Result<RepairFrame>{
    let mut boot=[0;512];file.read_exact_at(&mut boot,0)?;
    work_frame(&boot,length)
}
fn work_frame(boot:&[u8;512],length:u64)->io::Result<RepairFrame>{
    let view=framing(boot,length)?;
    let mut original_bpb=[0;45];
    original_bpb[..42].copy_from_slice(&boot[64..106]);
    original_bpb[42..].copy_from_slice(&boot[108..111]);
    Ok(RepairFrame{view,original_bpb})
}
impl RepairFrame{
    pub(super) fn same_volume(self,reopened:Self)->bool{
        self.view.length==reopened.view.length&&self.original_bpb==reopened.original_bpb
    }
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

// Only a precursor to the separate -n check. The fixed -p path may print
// repair decisions on stdout; keep none of that text and accept no stderr.
// A successful summary alone cannot supersede the monitor's actual exit.
pub(super) fn preen_summary(code:i32,out:&[u8],err:&[u8],frame:Framing)->bool{
    let Some(out)=out.strip_suffix(b"\n") else{return false;};
    let mut rows=out.split(|&b|b==b'\n');
    if !matches!(code,0|1)||!err.is_empty()||rows.next()!=Some(BANNER){return false;}
    let Some(last)=rows.next_back() else{return false;};
    let ending=if let Some((remaining,fixed))=pair(last,b"/image: files corrupted ",b", files fixed "){
        if code!=1||remaining!=0||fixed==0{return false;}
        let Some(summary)=rows.next_back() else{return false;};summary
    }else{last};
    counts(ending,b"/image: clean. directories ",frame).is_some()&&(code==1||rows.next().is_none())
}

#[cfg(test)]
mod repair_tests{
    use super::*;
    const GOOD:&[u8]=b"exfatprogs version : 1.2.9\n/image: clean. directories 1, files 0\n";
    const FIXED:&[u8]=b"exfatprogs version : 1.2.9\nuntrusted correction detail\n/image: clean. directories 2, files 0\n/image: files corrupted 0, files fixed 1\n";
    #[test]fn positive_preen_summary_does_not_turn_unresolved_or_missing_output_into_success(){
        let view=Framing{length:1<<20,sector:512,flags:0};
        assert!(preen_summary(0,GOOD,b"",view));assert!(preen_summary(1,FIXED,b"",view));
        assert!(preen_summary(1,GOOD,b"",view)); // a non-file fix need not emit file counters
        assert!(!preen_summary(0,FIXED,b"",view));assert!(!preen_summary(1,GOOD,b"error\n",view));
        assert!(!preen_summary(1,&FIXED[..FIXED.len()-1],b"",view));
        let unresolved=std::str::from_utf8(FIXED).unwrap().replace("corrupted 0","corrupted 1");
        assert!(!preen_summary(1,unresolved.as_bytes(),b"",view));
        for code in [2,3,4,8,16,32,128,137]{assert!(!preen_summary(code,GOOD,b"",view));}
        let flagged=Framing{flags:2,..view};assert_eq!(classify(0,GOOD,b"",flagged).unwrap().exit(),1);
    }
    #[test]fn same_volume_requires_the_verified_bpb_not_matching_length_alone(){
        let mut boot=[0u8;512];let n=3u64<<30;
        boot[..3].copy_from_slice(&[0xeb,0x76,0x90]);boot[3..11].copy_from_slice(b"EXFAT   ");
        boot[72..80].copy_from_slice(&(n/512).to_le_bytes());boot[105]=1;boot[108]=9;boot[110]=1;
        boot[510..].copy_from_slice(&[0x55,0xaa]);
        let original=work_frame(&boot,n).unwrap();
        boot[106]=2;boot[112]=30;
        assert!(original.same_volume(work_frame(&boot,n).unwrap()));
        for at in [64,80,92,96,100,109]{
            boot[at]^=1;assert!(!original.same_volume(work_frame(&boot,n).unwrap()));boot[at]^=1;
        }
        assert!(work_frame(&boot,n+512).is_err());
    }
}

#[cfg(test)]
#[path="../../tests/fs_tools/exfat.rs"]
mod tests;
