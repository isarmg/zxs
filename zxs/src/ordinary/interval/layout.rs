//! In-memory selection only. Rebuilt from the ordered authenticated FILE
//! predecessors. No persisted second map, synthesized bytes or FILE authority.
use super::types::SpanEdit;
use crate::fs::invalid;
use std::{collections::BTreeMap,io};

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) enum From{Base,Extent(usize),Zero}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct Piece{end:u64,from:From,source:u64} // map key is the logical beginning
pub(super) struct Layout{len:u64,parts:BTreeMap<u64,Piece>}
impl Layout{
    pub(super) fn base(len:u64)->Self{
        let mut parts=BTreeMap::new();if len!=0{parts.insert(0,Piece{end:len,from:From::Base,source:0});}
        Self{len,parts}
    }
    pub(super) fn len(&self)->u64{self.len}
    fn split(&mut self,at:u64)->io::Result<()>{
        let Some((&start,&p))=self.parts.range(..=at).next_back() else{return Ok(())};
        if start==at||p.end<=at{return Ok(());}
        let source=if p.from==From::Zero{0}else{p.source.checked_add(at-start).ok_or_else(||invalid("working slice source overflow"))?};
        self.parts.insert(start,Piece{end:at,..p});self.parts.insert(at,Piece{source,..p});Ok(())
    }
    fn discard(&mut self,start:u64,end:u64){
        let covered:Vec<u64>=self.parts.range(start..end).map(|(&k,_)|k).collect();
        for k in covered{self.parts.remove(&k);}
    }
    pub(super) fn apply(&mut self,prior:u64,e:SpanEdit,result:u64,which:usize)->io::Result<()>{
        if self.len!=prior||e.result(prior)?!=result{return Err(invalid("range map loses its authenticated prior size"));}
        if result<prior{self.split(result)?;self.discard(result,prior);}
        else if result>prior{self.parts.insert(prior,Piece{end:result,from:From::Zero,source:0});}
        if e.input.bytes!=0{
            let end=e.end();self.split(e.offset)?;self.split(end)?;self.discard(e.offset,end);
            self.parts.insert(e.offset,Piece{end,from:From::Extent(which),source:0});
        }
        self.len=result;Ok(())
    }
    pub(super) fn visit(&self,at:u64,n:u64,mut f:impl FnMut(From,u64,u64)->io::Result<()>)->io::Result<()>{
        let end=at.checked_add(n).filter(|e|*e<=self.len).ok_or_else(||invalid("read exceeds selected working revision"))?;
        if n==0{return Ok(());}
        let first=*self.parts.range(..=at).next_back().ok_or_else(||invalid("missing start of working material"))?.0;
        let mut pos=at;
        for (&lo,&p) in self.parts.range(first..end){
            if lo>pos||p.end<=pos{return Err(invalid("working map contains a gap or stale fragment"));}
            let hi=end.min(p.end);let off=if p.from==From::Zero{0}else{p.source.checked_add(pos-lo).ok_or_else(||invalid("working slice offset overflow"))?};
            f(p.from,off,hi-pos)?;pos=hi;
        }
        if pos!=end{return Err(invalid("missing end of working material"));}Ok(())
    }
}
#[cfg(test)]
mod tests{
    use super::*;
    use crate::{DraftClaim,LengthPolicy};
    fn edit(at:u64,len:u64,policy:LengthPolicy)->SpanEdit{
        SpanEdit::new(at,DraftClaim::new(len,crate::hash::digest(if len==0{b""}else{b"fixture"}).unwrap()).unwrap(),policy).unwrap()
    }
    #[test]
    fn superseded_ranges_truncation_then_growth_and_overflow_preserve_exact_selected_origins(){
        let mut l=Layout::base(10);
        l.apply(10,edit(2,6,LengthPolicy::KeepOrGrow),10,0).unwrap();
        l.apply(10,edit(3,2,LengthPolicy::Set(6)),6,1).unwrap();
        l.apply(6,edit(0,0,LengthPolicy::Set(11)),11,2).unwrap();
        l.apply(11,edit(8,1,LengthPolicy::KeepOrGrow),11,3).unwrap();
        let mut got=Vec::new();l.visit(1,10,|s,o,n|{got.push((s,o,n));Ok(())}).unwrap();
        assert_eq!(got,vec![(From::Base,1,1),(From::Extent(0),0,1),(From::Extent(1),0,2),
            (From::Extent(0),3,1),(From::Zero,0,2),(From::Extent(3),0,1),(From::Zero,0,2)]);
        assert!(l.visit(u64::MAX,1,|_,_,_|Ok(())).is_err());assert!(l.visit(11,1,|_,_,_|Ok(())).is_err());
    }
}
