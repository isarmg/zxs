//! Read exactly the queried interval from an original held body and its
//! canonical local index. The same FILE chunk scheme applies to a raw FULL
//! and to changed bytes inside one retained working extent.
use super::{content::{self,BLOCK},draft::types::DraftClaim};
use crate::fs::{invalid,stream};
use std::io::{self,Write};

const INDEX_BUFFER:usize=64*1024;
pub(super) struct Indexed<'a>{
    pub raw:&'a std::fs::File,pub index:&'a std::fs::File,pub header:Vec<u8>,
    pub raw_start:u64,pub link_start:u64,pub claim:DraftClaim,pub head:[u8;32],pub digest:[u8;32],
}
impl Indexed<'_>{
    fn verified_anchor(&self,first:Option<u64>,check:&impl Fn()->io::Result<()>)->io::Result<[u8;32]>{
        check()?;
        let expected_header=&self.header;let mut header=vec![0;expected_header.len()];
        stream::read_at(self.index,&mut header,0)?;
        if &header!=expected_header{return Err(invalid("range-selected original local node header differs"));}
        let mut h=crate::hash::Sha256::new();h.update(&header)?;
        let count=content::chunks(self.claim.bytes);
        if first.is_some_and(|i|i>=count){return Err(invalid("range beginning has no authentic content block"));}
        let wanted=first.and_then(|n|n.checked_sub(1));
        let mut anchor=if wanted.is_none(){Some(self.head)}else{None};
        let mut at=0u64;let mut buf=[0u8;INDEX_BUFFER];
        while at<count{
            let records=(count-at).min((INDEX_BUFFER/32) as u64);let n=records as usize*32;
            stream::read_at(self.index,&mut buf[..n],self.link_start+at*32)?;
            h.update(&buf[..n])?;
            if let Some(i)=wanted.filter(|i|*i>=at&&*i<at+records){
                let off=(i-at) as usize*32;let sampled:[u8;32]=buf[off..off+32].try_into().expect("one complete authenticated index row");
                anchor=Some(sampled);
            }
            at+=records;
        }
        check()?;
        if h.finish()!=self.digest{return Err(invalid("original formal-node index digest does not match its actual bytes"));}
        // No subsequent unchecked reread can replace this anchor.
        anchor.ok_or_else(||invalid("requested anchor not present in original index"))
    }
    pub(super) fn write_range(&self,offset:u64,length:u64,mut out:impl Write,check:impl Fn()->io::Result<()>)->io::Result<()>{
        let total=self.claim.bytes;
        let end=offset.checked_add(length).filter(|e|*e<=total).ok_or_else(||invalid("FULL range is outside the selected original file"))?;
        let mut proof=self.verified_anchor(if length==0{None}else{Some(offset/BLOCK)},&check)?;
        if length==0{return Ok(());}
        let header=self.link_start;
        let mut buf=vec![0;total.min(BLOCK) as usize];let last=(end-1)/BLOCK;
        for i in offset/BLOCK..=last{
            let base=i*BLOCK;let count=(total-base).min(BLOCK) as usize;let part=&mut buf[..count];
            stream::read_at(self.raw,part,self.raw_start+base)?;
            let mut next=[0;32];stream::read_at(self.index,&mut next,header+i*32)?;
            if content::link(i,count,crate::hash::digest(part)?,next)?!=proof{
                return Err(invalid("range block or its next link does not follow the authenticated original"));
            }
            if i+1==content::chunks(total)&&next!=content::end(self.claim)?{return Err(invalid("wrong formal FULL terminal link"));}
            check()?;
            let lo=offset.saturating_sub(base) as usize;let hi=(end-base).min(count as u64) as usize;
            out.write_all(&part[lo..hi])?;proof=next;
        }
        check()
    }
}
