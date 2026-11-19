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
#[cfg(test)]
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) enum Source{Full,Pack(usize),Zero}
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
    #[cfg(test)]
    fn resolve<'a>(full: u64, layers: impl IntoIterator<Item=&'a Map>, at: u64, len: u64,
        mut emit: impl FnMut(Source, u64, u64)->io::Result<()>) -> io::Result<()> {
        let file=crate::FileId([1;16]); let op=crate::OperationId([2;16]);
        let mut view=Index::full(Material::declared(file,op,0,full)?)?;
        for (i,m) in layers.into_iter().enumerate() {
            let slot=u32::try_from(i+1).map_err(|_|invalid("fixture ordinal overflow"))?;
            view=m.flatten(&view,Material::declared(file,op,slot,m.changed)?)?;
        }
        view.visit(at,len,|leaf,off,n|emit(match leaf {
            Leaf::Zero=>Source::Zero, Leaf::Data(x) if x.slot()==0=>Source::Full,
            Leaf::Data(x)=>Source::Pack((x.slot()-1) as usize),
        },off,n))
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
mod range_plan_tests{
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
        let mut parts=Vec::new();Map::resolve(10,[&first,&second],2,7,|s,o,n|{parts.push((s,o,n));Ok(())}).unwrap();
        assert_eq!(parts,[(Source::Pack(0),0,1),(Source::Pack(1),0,1),(Source::Full,4,1),(Source::Zero,0,4)]);
        let mut bytes=Vec::new();Map::resolve(10,[&first,&second],0,9,|s,o,n|{
            if s==Source::Zero{bytes.resize(bytes.len()+n as usize,0);}else{
                let src:&[u8]=match s{Source::Pack(0)=>b"XY",Source::Pack(1)=>b"Z",Source::Full=>b"abcdefghij",_=>unreachable!()};
                bytes.extend_from_slice(&src[o as usize..(o+n)as usize]);}Ok(())
        }).unwrap();assert_eq!(bytes,b"abXZe\0\0\0\0");
        // Short reads map dense offsets in each independent original pack,
        // not into the larger FULL or the other generation's dense bytes.
        let mut middle=Vec::new();Map::resolve(10,[&first],3,1,|s,o,n|{middle.push((s,o,n));Ok(())}).unwrap();
        assert_eq!(middle,[(Source::Pack(0),1,1)]);
        assert!(Map::resolve(9,[&first,&second],0,0,|_,_,_|panic!("bad bound emitted bytes")).is_err());
        assert!(Map::resolve(10,[&second],0,9,|_,_,_|panic!("skipped parent emitted bytes")).is_err());
        assert!(Map::resolve(10,[&first,&second],9,1,|_,_,_|panic!("past end emitted bytes")).is_err());
        let empty=map(9,0,0,&[]);assert!(Map::resolve(10,[&first,&empty],0,0,|_,_,_|unreachable!()).is_err());
    }
    #[test]
    fn deep_overlap_uses_the_right_original_dense_offset_and_only_the_selected_ancestry(){
        // F0=abcdefghij; I1=abXYe, I2=abXZe + four zeroes;
        // I3 masks offsets 2..5; I4 masks 3..4; I5 truncates to 7 then I6 grows to 12.
        let maps=[map(10,5,2,&[(1,2),(2,2),(1,1)]),map(5,9,1,&[(1,3),(2,1),(1,1),(3,4)]),
            map(9,9,3,&[(1,2),(2,3),(1,4)]),map(9,9,1,&[(1,3),(2,1),(1,5)]),
            map(9,7,0,&[(1,7)]),map(7,12,0,&[(1,7),(3,5)])];
        let ranges=|n:usize,off,len|{let mut v=Vec::new();Map::resolve(10,&maps[..n],off,len,|s,o,l|{v.push((s,o,l));Ok(())}).unwrap();v};
        assert_eq!(ranges(4,2,4),[(Source::Pack(2),0,1),(Source::Pack(3),0,1),(Source::Pack(2),2,1),(Source::Zero,0,1)]);
        assert_eq!(ranges(6,0,12),[(Source::Full,0,2),(Source::Pack(2),0,1),(Source::Pack(3),0,1),(Source::Pack(2),2,1),(Source::Zero,0,7)]);
        assert_eq!(ranges(2,2,3),[(Source::Pack(0),0,1),(Source::Pack(1),0,1),(Source::Full,4,1)]);
        assert_eq!(ranges(0,8,2),[(Source::Full,8,2)]);
        assert!(Map::resolve(10,&maps[..],u64::MAX,2,|_,_,_|panic!("bad range emitted bytes")).is_err());
    }
    #[test]
    fn ordinals_are_not_byte_tags_and_a_fully_overwritten_layer_cannot_hide_a_broken_edge(){
        let layers:Vec<_>=(0..300).map(|_|map(3,3,1,&[(1,1),(2,1),(1,1)])).collect();
        let mut got=Vec::new();Map::resolve(3,&layers,0,3,|s,o,n|{got.push((s,o,n));Ok(())}).unwrap();
        assert_eq!(got,[(Source::Full,0,1),(Source::Pack(299),0,1),(Source::Full,2,1)]);
        let corrupt=map(4,3,3,&[(2,3)]);let cover=map(3,3,3,&[(2,3)]);
        assert!(Map::resolve(3,layers.iter().chain([&corrupt,&cover]),0,3,|_,_,_|panic!("broken ancestor emitted bytes")).is_err());
    }

}
