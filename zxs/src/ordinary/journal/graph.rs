//! At most the original FULL and two immediate, ordered formal children.
//! This projection proves ancestry at a prefix. Only the live native cut
//! may decide *where* that selected immutable original can be opened.
use super::{Commit,Delta,Frame,Journal,cycle::Parent};
use crate::{FileId,NodeId,fs::invalid,ordinary::custody::transfer::ChildRoles};
use std::io;

#[derive(Clone,Copy)]
pub(in crate::ordinary) struct Graph<'a>{full:&'a Commit,children:ChildRoles<&'a Delta>}
impl<'a> Graph<'a>{
    pub(in crate::ordinary) fn at(c:&Frame,f:&'a Commit,children:&[&'a Delta])->io::Result<Self>{
        let items=match children{[]=>ChildRoles::None,[one]=>ChildRoles::One(*one),
            [one,two]=>ChildRoles::Two(*one,*two),_=>return Err(invalid("unsupported depth of retained FILE history"))};
        Self::from_roles(c,f,items)
    }
    pub(in crate::ordinary) fn from_roles(c:&Frame,f:&'a Commit,items:ChildRoles<&'a Delta>)->io::Result<Self>{
        if c.seq>=f.seq||f.original.file!=c.create.file||f.original.history!=c.create.history||f.original.creation!=c.digest{
            return Err(invalid("wrong original FILE or first FULL in retained formal graph"));}
        let(mut seq,mut parent)=(f.seq,Parent::full(f));
        for d in items.iter().copied(){
            if d.seq<=seq||!parent.precedes(d){return Err(invalid("missing, reversed or substituted immediate formal FILE parent"));}
            seq=d.seq;parent=Parent::incremental(d);
        }Ok(Self{full:f,children:items})
    }
    pub(in crate::ordinary) fn hashes(self)->ChildRoles<[u8;32]>{self.children.map(|d|d.digest)}
    pub(in crate::ordinary) fn full(self)->&'a Commit{self.full}
    pub(in crate::ordinary) fn children(self)->ChildRoles<&'a Delta>{self.children}
    pub(in crate::ordinary) fn tip(self)->Parent{match self.children{
        ChildRoles::None=>Parent::full(self.full),ChildRoles::One(d)|ChildRoles::Two(_,d)=>Parent::incremental(d),
    }}
    /// Only F0 and its first confirmed child may be the parent of a new
    /// working cycle. A complete-but-unconfirmed original is not a child
    /// here. A still-open, already published submission must match the exact
    /// next original at this tip, not somebody else's last node/receipt.
    pub(in crate::ordinary) fn working_parent(self,want:Parent,published:Option<&Delta>)->io::Result<()>{
        let selected=self.through(want.node)?;let i=selected.children.len();
        if selected.tip()!=want||i>=2{return Err(invalid("unsupported or different original working parent"));}
        let ok=match published{
            None=>i==self.children.len(),
            Some(d)=>self.children.len()==i+1&&self.children.ordinal(i).copied()==Some(d)&&want.precedes(d),
        };
        if !ok{return Err(invalid("original working cycle is not adjacent to the current confirmed frontier"));}Ok(())
    }
    pub(in crate::ordinary) fn through(self,node:NodeId)->io::Result<Self>{
        let selected=if node==self.full.original.node{ChildRoles::None}
        else if let Some(n)=self.children.first().copied().filter(|d|d.original.node==node){ChildRoles::One(n)}
        else if self.children.second().is_some_and(|d|d.original.node==node){self.children}
        else{return Err(io::Error::new(io::ErrorKind::NotFound,"no such published node in this original FILE graph"));};
        Ok(Self{full:self.full,children:selected})
    }
}
impl<'a> Graph<'a>{
    /// Resolve positions only against the exact authenticated slice on which
    /// this index was built. Used before admitting the next native decision
    /// as well as by the final confirmed reader.
    pub(super) fn indexed(c:&Frame,f:&'a Commit,ds:&'a[super::Decision],ids:&[usize])->io::Result<Self>{
        let get=|i:usize|ds.get(i).ok_or_else(||invalid("missing indexed original INCR"))?.delta();
        let children=match ids{[]=>ChildRoles::None,[i]=>ChildRoles::One(get(*i)?),
            [i,j]=>ChildRoles::Two(get(*i)?,get(*j)?),_=>return Err(invalid("unsupported number of original children"))};
        Self::from_roles(c,f,children)
    }
}
impl Journal{
    /// Frozen subset for an actual earlier custody frame. Limit the *same*
    /// confirmed FILE index by original ordinal; newer native children never
    /// enlarge an older handoff. No extra full prefix scan per physical role.
    pub(in crate::ordinary) fn handoff_graph(&self,m:&super::Moved)->io::Result<(super::Placement<'_>,Graph<'_>)>{
        let before=self.before_handoff(m)?;let id=m.original.file;
        let at=m.seq.checked_sub(1).and_then(|n|usize::try_from(n).ok()).ok_or_else(||invalid("invalid original handoff sequence"))?;
        let f=self.head(id).filter(|v|v.seq<m.seq).ok_or_else(||invalid("original FULL did not precede handoff"))?;
        let ds=self.decisions.get(..at).ok_or_else(||invalid("unavailable original custody prefix"))?;
        let all=self.deltas.get(&id).map(Vec::as_slice).unwrap_or(&[]);let stop=all.partition_point(|&n|n<at);
        if self.work_at(id,at)?.last().is_some(){return Err(invalid("a dirty original FILE cannot be a frozen transfer"));}
        let g=Graph::indexed(before.birth(),f,ds,&all[..stop])?;Ok((before,g))
    }
    /// The index contains only positions of actual confirmed frames. Do not
    /// consult the pending allocation/seal or treat an unavailable node as F0.
    pub(in crate::ordinary) fn actual_graph(&self,id:FileId)->io::Result<Graph<'_>>{
        let c=self.file(id).ok_or_else(||invalid("missing original FILE birth at formal cutoff"))?;
        let f=self.head(id).ok_or_else(||invalid("no confirmed first FULL at formal cutoff"))?;
        let ids=self.deltas.get(&id).map(Vec::as_slice).unwrap_or(&[]);
        Graph::indexed(c,f,&self.decisions,ids)
    }
}
