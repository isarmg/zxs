//! Authenticate the small existing node index once per cold range, retaining
//! only a *sampled* prefix commitment from that exact digest pass. Each
//! subsequently read body block + next link must prove themselves against
//! that authenticated anchor before any byte of the block leaves the process.
use super::physical::HistoryBody;
use crate::{fs::{invalid,stream},ordinary::content::{self,BLOCK}};
use std::io::{self,Write};

const INDEX_BUFFER:usize=64*1024;
impl HistoryBody{
    fn verified_anchor(&self,first:Option<u64>)->io::Result<[u8;32]>{
        self.unchanged()?;
        let o=&self.original;let expected_header=o.node_header();let mut header=vec![0;expected_header.len()];
        stream::read_at(&self.node.file,&mut header,0)?;
        if header!=expected_header{return Err(invalid("range-selected original local node header differs"));}
        let mut h=crate::hash::Sha256::new();h.update(&header)?;
        let count=content::chunks(o.content.bytes);
        if first.is_some_and(|i|i>=count){return Err(invalid("range beginning has no authentic content block"));}
        let wanted=first.and_then(|n|n.checked_sub(1));
        let mut anchor=if wanted.is_none(){Some(o.chain)}else{None};
        let mut at=0u64;let mut buf=[0u8;INDEX_BUFFER];
        while at<count{
            let records=(count-at).min((INDEX_BUFFER/32) as u64);let n=records as usize*32;
            stream::read_at(&self.node.file,&mut buf[..n],header.len() as u64+at*32)?;
            h.update(&buf[..n])?;
            if let Some(i)=wanted.filter(|i|*i>=at&&*i<at+records){
                let off=(i-at) as usize*32;let sampled:[u8;32]=buf[off..off+32].try_into().expect("one complete authenticated index row");
                anchor=Some(sampled);
            }
            at+=records;
        }
        self.unchanged()?;
        if h.finish()!=self.index_digest{return Err(invalid("original formal-node index digest does not match its actual bytes"));}
        // No subsequent unchecked reread can replace this anchor.
        anchor.ok_or_else(||invalid("requested anchor not present in original index"))
    }
    pub(super) fn write_range(&self,offset:u64,length:u64,mut out:impl Write)->io::Result<()>{
        let o=&self.original;let total=o.content.bytes;
        let end=offset.checked_add(length).filter(|e|*e<=total).ok_or_else(||invalid("FULL range is outside the selected original file"))?;
        let mut proof=self.verified_anchor(if length==0{None}else{Some(offset/BLOCK)})?;
        if length==0{return Ok(());}
        let header=o.node_header().len() as u64;
        let mut buf=vec![0;total.min(BLOCK) as usize];let last=(end-1)/BLOCK;
        for i in offset/BLOCK..=last{
            let base=i*BLOCK;let count=(total-base).min(BLOCK) as usize;let part=&mut buf[..count];
            stream::read_at(&self.raw.file,part,base)?;
            let mut next=[0;32];stream::read_at(&self.node.file,&mut next,header+i*32)?;
            if content::link(i,count,crate::hash::digest(part)?,next)?!=proof{
                return Err(invalid("range block or its next link does not follow the authenticated original"));
            }
            if i+1==content::chunks(total)&&next!=content::end(o.content)?{return Err(invalid("wrong formal FULL terminal link"));}
            self.unchanged()?;
            let lo=offset.saturating_sub(base) as usize;let hi=(end-base).min(count as u64) as usize;
            out.write_all(&part[lo..hi])?;proof=next;
        }
        self.unchanged()
    }
}
