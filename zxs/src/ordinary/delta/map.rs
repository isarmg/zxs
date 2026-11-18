//! Canonical logical coverage of a formal INCR. Its only data sources are the
//! authenticated immutable parent, dense bytes in this version, and declared
//! zero. Working names, routes and current path/HEAD cannot appear here.
use crate::{fs::invalid,ordinary::interval::{layout::{From,Layout},types::Saved}};
use std::io;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) enum Part{Parent,Changed,Zero}
impl Part{fn byte(self)->u8{match self{Self::Parent=>1,Self::Changed=>2,Self::Zero=>3}}}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct Run{kind:Part,len:u64}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Map{parent_length:u64,runs:Vec<Run>,pub changed:u64,pub logical:u64}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) enum Source{Full,First,Second,Zero}
impl Map{
    pub(in crate::ordinary) fn derive<'a>(base:u64,spans:impl Iterator<Item=&'a Saved>)->io::Result<Self>{
        let mut l=Layout::base(base);let mut count=0;
        for s in spans{let o=&s.original;l.apply(o.predecessor.length,o.edit,o.result,count)?;count+=1;}
        if count==0{return Err(invalid("no confirmed change to publish as an INCR"));}
        Self::from_layout(&l,base)
    }
    fn from_layout(l:&Layout,base:u64)->io::Result<Self>{
        let mut result=Self{parent_length:base,runs:Vec::new(),logical:l.len(),changed:0};let mut pos=0u64;
        l.visit(0,l.len(),|source,offset,n|{
            let end=pos.checked_add(n).ok_or_else(||invalid("formal map overflows"))?;
            let kind=match source{
                From::Base if offset==pos&&end<=base=>Part::Parent,
                From::Base=>return Err(invalid("formal INCR cannot redirect a parent interval")),
                From::Extent(_)=>{result.changed=result.changed.checked_add(n).ok_or_else(||invalid("formal changed length overflow"))?;Part::Changed},
                From::Zero=>Part::Zero,
            };
            if let Some(last)=result.runs.last_mut().filter(|p|p.kind==kind){last.len+=n;}else{result.runs.push(Run{kind,len:n});}
            pos=end;Ok(())
        })?;
        if pos!=l.len(){return Err(invalid("incomplete original map for formal INCR"));}Ok(result)
    }
    pub(super) fn checked_bytes(rows:u32)->io::Result<usize>{
        // Each genuine source operation introduces at most three logical cuts:
        // two changed-interval endpoints and one explicit final size.
        if rows as usize>3*crate::ordinary::journal::MAX_DECISIONS+1{return Err(invalid("formal map exceeds original decision bound"));}
        Ok(4+rows as usize*9)
    }
    pub(super) fn decode(raw:&[u8],rows:u32,parent:u64,logical:u64,changed:u64)->io::Result<Self>{
        if raw.len()!=Self::checked_bytes(rows)?{return Err(invalid("wrong local INCR map size"));}
        let mut rd=crate::ordinary::wire::In(raw);if rd.u32()?!=rows{return Err(invalid("wrong local INCR row count"));}
        let mut runs:Vec<Run>=Vec::with_capacity(rows as usize);let mut pos=0u64;let mut dense=0u64;
        for _ in 0..rows{
            let kind=match rd.byte()?{1=>Part::Parent,2=>Part::Changed,3=>Part::Zero,_=>return Err(invalid("unknown immutable INCR source"))};
            let len=rd.u64()?;let end=pos.checked_add(len).filter(|v|*v<=logical).ok_or_else(||invalid("INCR local map exceeds logical size"))?;
            if len==0||runs.last().is_some_and(|p|p.kind==kind)||(kind==Part::Parent&&end>parent){return Err(invalid("noncanonical or impossible INCR map interval"));}
            if kind==Part::Changed{dense=dense.checked_add(len).filter(|n|*n<=changed).ok_or_else(||invalid("bad dense interval coverage"))?;}
            runs.push(Run{kind,len});pos=end;
        }
        rd.end()?;if pos!=logical||dense!=changed{return Err(invalid("incomplete local immutable INCR map"));}
        Ok(Self{parent_length:parent,runs,logical,changed})
    }
    pub(super) fn rows(&self)->u32{self.runs.len().try_into().expect("bounded by the authenticated SYSTEM frame count")}
    pub(super) fn put(&self,b:&mut Vec<u8>){
        b.extend_from_slice(&self.rows().to_be_bytes());
        for r in &self.runs{b.push(r.kind.byte());b.extend_from_slice(&r.len.to_be_bytes());}
    }
    pub(super) fn digest(&self)->io::Result<[u8;32]>{
        let mut b=b"ZXS/FORMAL/INCR/MAP\0".to_vec();b.extend_from_slice(&self.logical.to_be_bytes());b.extend_from_slice(&self.changed.to_be_bytes());self.put(&mut b);
        crate::hash::digest(&b)
    }
    /// Resolve one bounded graph, never rebase child Parent runs onto FULL.
    /// Offsets for dense packs remain relative to their *own* packed stream.
    pub(super) fn resolve(&self,older:Option<&Map>,full:u64,at:u64,len:u64,
        mut emit:impl FnMut(Source,u64,u64)->io::Result<()>)->io::Result<()>{
        if self.parent_length!=older.map_or(full,|p|p.logical)||older.is_some_and(|p|p.parent_length!=full){
            return Err(invalid("immutable map does not cover the actual immediate parent's logical length"));}
        self.visit(at,len,|part,off,n|match(part,older){
            (Part::Zero,_)=>emit(Source::Zero,0,n),(Part::Changed,None)=>emit(Source::First,off,n),
            (Part::Changed,Some(_))=>emit(Source::Second,off,n),(Part::Parent,None)=>emit(Source::Full,off,n),
            (Part::Parent,Some(p))=>p.visit(off,n,|source,p_at,p_len|emit(match source{
                Part::Parent=>Source::Full,Part::Changed=>Source::First,Part::Zero=>Source::Zero
            },p_at,p_len)),
        })
    }
    pub(super) fn visit(&self,at:u64,len:u64,mut f:impl FnMut(Part,u64,u64)->io::Result<()>)->io::Result<()>{
        let end=at.checked_add(len).filter(|v|*v<=self.logical).ok_or_else(||invalid("range outside the exact incremental version"))?;
        let mut pos=0;let mut changed=0;
        for run in &self.runs{
            if pos>=end{break;}
            let hi=pos+run.len;let lo=pos.max(at);let stop=hi.min(end);
            if lo<stop{
                let off=match run.kind{Part::Parent=>lo,Part::Changed=>changed+(lo-pos),Part::Zero=>0};
                f(run.kind,off,stop-lo)?;
            }
            if run.kind==Part::Changed{changed+=run.len;}pos=hi;
        }Ok(())
    }
}

#[cfg(test)]
mod two_layer_tests{
    use super::*;
    fn map(parent:u64,logical:u64,changed:u64,runs:&[(u8,u64)])->Map{
        let mut b=(runs.len()as u32).to_be_bytes().to_vec();
        for &(tag,n) in runs{b.push(tag);b.extend_from_slice(&n.to_be_bytes());}
        Map::decode(&b,runs.len()as u32,parent,logical,changed).unwrap()
    }
    #[test]
    fn second_generation_inherits_changes_only_from_its_immediate_parent_and_never_untruncates_full(){
        let first=map(10,5,2,&[(1,2),(2,2),(1,1)]);
        let second=map(5,9,1,&[(1,3),(2,1),(1,1),(3,4)]);
        let mut parts=Vec::new();second.resolve(Some(&first),10,2,7,|s,o,n|{parts.push((s,o,n));Ok(())}).unwrap();
        assert_eq!(parts,[(Source::First,0,1),(Source::Second,0,1),(Source::Full,4,1),(Source::Zero,0,4)]);
        let mut bytes=Vec::new();second.resolve(Some(&first),10,0,9,|s,o,n|{
            if s==Source::Zero{bytes.resize(bytes.len()+n as usize,0);}else{
                let src:&[u8]=match s{Source::First=>b"XY",Source::Second=>b"Z",Source::Full=>b"abcdefghij",Source::Zero=>unreachable!()};
                bytes.extend_from_slice(&src[o as usize..(o+n)as usize]);}Ok(())
        }).unwrap();assert_eq!(bytes,b"abXZe\0\0\0\0");
        // Short reads map dense offsets in each independent original pack,
        // not into the larger FULL or the other generation's dense bytes.
        let mut middle=Vec::new();first.resolve(None,10,3,1,|s,o,n|{middle.push((s,o,n));Ok(())}).unwrap();
        assert_eq!(middle,[(Source::First,1,1)]);
        assert!(second.resolve(Some(&first),9,0,0,|_,_,_|panic!("bad bound emitted bytes")).is_err());
        assert!(second.resolve(None,10,0,9,|_,_,_|panic!("skipped parent emitted bytes")).is_err());
        assert!(second.resolve(Some(&first),10,9,1,|_,_,_|panic!("past end emitted bytes")).is_err());
        let empty=map(9,0,0,&[]);assert!(empty.resolve(Some(&first),10,0,0,|_,_,_|unreachable!()).is_err());
    }
}
