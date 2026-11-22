//! Canonical logical coverage of a formal INCR. Its only data sources are the
//! authenticated immutable parent, dense bytes in this version, and declared
//! zero. Working names, routes and current path/HEAD cannot appear here.
use crate::{fs::invalid,ordinary::interval::{layout::{From,Layout},types::Saved}};
use crate::ordinary::flat::{Build, Index, Leaf, Material};
use std::io;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) enum Part{Parent,Changed,Zero}
impl Part{fn byte(self)->u8{match self{Self::Parent=>1,Self::Changed=>2,Self::Zero=>3}}}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct Run{kind:Part,len:u64}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Map{parent_length:u64,runs:Vec<Run>,pub changed:u64,pub logical:u64}
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
    /// Interpret this old-format local map exactly once, when its real FILE
    /// operands have been admitted. The resulting index names terminal
    /// materials only. No Parent run survives to be resolved on each read.
    pub(super) fn flatten(&self, parent: &Index, changes: Material) -> io::Result<Index> {
        if parent.len() != self.parent_length || changes.file() != parent.file() || changes.size() != self.changed {
            return Err(invalid("wrong material or immediate parent for a direct FILE index"));
        }
        let mut out = Build::new(parent.file())?;
        self.visit(0, self.logical, |part, off, n| match part {
            Part::Parent => out.inherit(parent, off, n),
            Part::Changed => out.add(Leaf::Data(changes), off, n),
            Part::Zero => out.add(Leaf::Zero, 0, n),
        })?;
        out.finish(self.logical)
    }

    // Translate synthetic, ordered fixture slots back to its expected old
    // range labels. Production maps only ever receive original FILE/Op keys.
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
#[path = "../../../tests/unit/attached/ordinary/delta/map/range_plan_tests.rs"]
mod range_plan_tests;

#[cfg(test)]
include!("../../../tests/unit/attached/ordinary/delta/map/items.rs");
